use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::fs;
use std::path::PathBuf;
use std::ptr::null_mut;

use anyhow::anyhow;
use askama::Template;
use reaper_medium::ProjectContext::CurrentProject;
use reaper_medium::{ChunkCacheHint, CommandId, ProjectContext, ProjectRef, ReaProject, Reaper};
use tempdir::TempDir;
use tracing::*;

use audiocloud_api::change::{ModifySessionSpec, PlaySession, RenderSession};
use audiocloud_api::cloud::apps::SessionSpec;
use audiocloud_api::cloud::domains::InstanceRouting;
use audiocloud_api::newtypes::{AppMediaObjectId, AppSessionId, FixedId, FixedInstanceId, MixerId, TrackId};
use audiocloud_api::session::{SessionConnection, SessionFixedInstance, SessionFlowId, SessionMixer, SessionTrack};

use crate::audio_engine::fixed_instance::AudioEngineFixedInstance;
use crate::audio_engine::media_track::AudioEngineMediaTrack;
use crate::audio_engine::mixer::AudioEngineMixer;

#[derive(Debug)]
pub struct AudioEngineProject {
    id:              AppSessionId,
    project:         ReaProject,
    tracks:          HashMap<TrackId, AudioEngineMediaTrack>,
    fixed_instances: HashMap<FixedId, AudioEngineFixedInstance>,
    mixers:          HashMap<MixerId, AudioEngineMixer>,
    spec:            SessionSpec,
    media_root:      PathBuf,
    temp_dir:        TempDir,
}

const CMD_SWITCH_NEXT_PROJECT_TAB: CommandId = CommandId::new(40861);
const CMD_CLOSE_PROJECT_TAB: CommandId = CommandId::new(40860);
const CMD_TRANSPORT_RECORD: CommandId = CommandId::new(1013);

#[derive(Template)]
#[template(path = "audio_engine/project.txt")]
struct AudioEngineProjectTemplate<'a> {
    spec:       &'a SessionSpec,
    session_id: &'a AppSessionId,
    media_root: String,
}

impl AudioEngineProject {
    pub fn new(id: AppSessionId,
               temp_dir: TempDir,
               spec: SessionSpec,
               instances: HashMap<FixedInstanceId, InstanceRouting>,
               media: HashMap<AppMediaObjectId, String>)
               -> anyhow::Result<Self> {
        let reaper = Reaper::get();
        let media_root = temp_dir.path().join("media");
        let session_path = temp_dir.path().join("session.rpp");
        fs::write(&session_path,
                  AudioEngineProjectTemplate { spec:       &spec,
                                               session_id: &id,
                                               media_root: media_root.to_string_lossy().to_string(), }.render()?)?;

        unsafe {
            let path_as_cstr = CString::new(format!("noprompt:{}", session_path.to_string_lossy()))?;
            reaper.low().Main_openProject(path_as_cstr.as_ptr());
        }

        let project = reaper.enum_projects(ProjectRef::Current, 0)
                            .ok_or_else(|| anyhow!("No current project even though we just opened one"))?
                            .project;

        let mut rv = Self { id,
                            project,
                            tracks: Default::default(),
                            fixed_instances: Default::default(),
                            mixers: Default::default(),
                            spec: Default::default(),
                            media_root,
                            temp_dir };

        rv.set_spec(spec, instances, media)?;

        Ok(rv)
    }

    pub fn delete(&self) -> anyhow::Result<()> {
        Ok(())
    }

    pub fn context(&self) -> ProjectContext {
        ProjectContext::Proj(self.project)
    }

    pub fn media_root_dir(&self) -> PathBuf {
        self.media_root.clone()
    }

    pub fn focus(&self) -> anyhow::Result<()> {
        let reaper = Reaper::get();
        let mut first = None;
        loop {
            if let Some(enumerated) = reaper.enum_projects(ProjectRef::Current, 0) {
                match first {
                    None => first = Some(enumerated.project),
                    Some(x) => {
                        if x == enumerated.project {
                            return Err(anyhow!("Project not found"));
                        }
                    }
                }

                if enumerated.project != self.project {
                    reaper.main_on_command_ex(CMD_SWITCH_NEXT_PROJECT_TAB, 0, CurrentProject);
                } else {
                    break;
                }
            }
        }

        Ok(())
    }

    pub fn flows_to<'a>(&'a self, flow: &'a SessionFlowId) -> impl Iterator<Item = &SessionConnection> + 'a {
        self.spec.connections.values().filter(move |conn| &conn.to == flow)
    }

    pub fn fixed_input_track_index(&self, fixed_id: &FixedId) -> Option<usize> {
        self.track_index(SessionFlowId::FixedInstanceInput(fixed_id.clone()))
    }

    pub fn track_index(&self, flow_id: SessionFlowId) -> Option<usize> {
        let reaper = Reaper::get();
        let mut index = 0;
        let flow_name = flow_id.to_string();

        while let Some(track) = reaper.get_track(ProjectContext::Proj(self.project), index) {
            let matches =
                unsafe { reaper.get_set_media_track_info_get_name(track, |name| name.to_str() == flow_name.as_str()) };

            if matches.unwrap_or(false) {
                return Some(index as usize);
            }

            index += 1;
        }

        None
    }
}

impl Drop for AudioEngineProject {
    fn drop(&mut self) {
        if self.project.as_ptr() != null_mut() {
            let reaper = Reaper::get();
            if let Ok(_) = self.focus() {
                reaper.main_on_command_ex(CMD_CLOSE_PROJECT_TAB, 0, self.context());
            } else {
                warn!("Project could not be focused for closing");
            }
        }
    }
}

impl AudioEngineProject {
    pub fn add_track(&mut self,
                     id: TrackId,
                     spec: SessionTrack,
                     media: &HashMap<AppMediaObjectId, String>)
                     -> anyhow::Result<()> {
        self.tracks.insert(id.clone(),
                           AudioEngineMediaTrack::new(self, self.id.app_id.clone(), id.clone(), spec, media)?);

        Ok(())
    }

    pub fn delete_track(&mut self, id: &TrackId) {
        if let Some(track) = self.tracks.remove(id) {
            track.delete(self.context());
        }
    }

    pub fn add_fixed_instance(&mut self,
                              fixed_id: FixedId,
                              spec: SessionFixedInstance,
                              instances: &HashMap<FixedInstanceId, InstanceRouting>)
                              -> anyhow::Result<()> {
        let routing = instances.get(&spec.instance_id).cloned();

        self.fixed_instances.insert(fixed_id.clone(),
                                    AudioEngineFixedInstance::new(self, fixed_id.clone(), spec, routing)?);

        Ok(())
    }

    pub fn delete_fixed_instance(&mut self, fixed_id: FixedId) -> anyhow::Result<()> {
        if let Some(fixed) = self.fixed_instances.remove(&fixed_id) {
            fixed.delete(self.context());
        }

        Ok(())
    }

    pub fn add_mixer(&mut self, mixer_id: MixerId, spec: SessionMixer) -> anyhow::Result<()> {
        self.mixers
            .insert(mixer_id.clone(), AudioEngineMixer::new(self, mixer_id.clone(), spec)?);

        Ok(())
    }

    fn delete_mixer(&mut self, mixer_id: &MixerId) {
        if let Some(mixer) = self.mixers.remove(mixer_id) {
            mixer.delete(self.context());
        }
    }

    pub fn render(&mut self, render: RenderSession) -> anyhow::Result<()> {
        let reaper = Reaper::get();
        reaper.main_on_command_ex(CMD_TRANSPORT_RECORD, 0, self.context());
        Ok(())
    }

    pub fn play(&mut self, play: PlaySession) -> anyhow::Result<()> {
        let reaper = Reaper::get();
        reaper.on_play_button_ex(self.context());
        Ok(())
    }

    pub fn stop(&mut self) -> anyhow::Result<()> {
        let reaper = Reaper::get();
        reaper.on_stop_button_ex(self.context());
        Ok(())
    }

    pub fn set_spec(&mut self,
                    spec: SessionSpec,
                    instances: HashMap<FixedInstanceId, InstanceRouting>,
                    media: HashMap<AppMediaObjectId, String>)
                    -> anyhow::Result<()> {
        self.stop()?;
        self.clear();

        for (track_id, track_spec) in spec.tracks.clone() {
            self.add_track(track_id, track_spec, &media)?;
        }

        for (fixed_id, fixed_spec) in spec.fixed.clone() {
            self.add_fixed_instance(fixed_id, fixed_spec, &instances)?;
        }

        // skip dynamic instances, not supported yet

        for (mixer_id, mixer_spec) in spec.mixers.clone() {
            self.add_mixer(mixer_id, mixer_spec)?;
        }

        self.spec = spec;

        self.update_all_state_chunks()?;

        Ok(())
    }

    fn update_all_state_chunks(&mut self) -> anyhow::Result<()> {
        for track in self.tracks.values() {
            track.update_state_chunk(self)?;
        }

        for instance in self.fixed_instances.values() {
            instance.update_state_chunk(self)?;
        }

        for mixer in self.mixers.values() {
            mixer.update_state_chunk(self)?;
        }

        Ok(())
    }

    pub fn modify_spec(&mut self,
                       transaction: Vec<ModifySessionSpec>,
                       instances: HashMap<FixedInstanceId, InstanceRouting>,
                       media_ready: HashMap<AppMediaObjectId, String>)
                       -> anyhow::Result<()> {
        let current_spec = self.spec.clone();
        let mut dirty_flows = HashSet::new();

        for item in transaction {
            if let Err(err) = self.modify_spec_one(item, &instances, &media_ready, &mut dirty_flows) {
                warn!(%err, "failed to execute transaction, rolling back");
                return Ok(self.set_spec(current_spec, instances, media_ready)?);
            }
        }

        for flow_id in dirty_flows {
            self.update_track_chunk(&flow_id)?;
        }

        Ok(())
    }

    fn update_track_chunk(&self, flow_id: &SessionFlowId) -> anyhow::Result<()> {
        let chunk = match flow_id {
            SessionFlowId::MixerInput(mixer_id) => self.mixers
                                                       .get(mixer_id)
                                                       .ok_or_else(|| anyhow!("Mixer {mixer_id} not found"))?
                                                       .get_input_state_chunk(self)?,
            SessionFlowId::MixerOutput(mixer_id) => self.mixers
                                                        .get(mixer_id)
                                                        .ok_or_else(|| anyhow!("Mixer {mixer_id} not found"))?
                                                        .get_output_state_chunk(self)?,
            SessionFlowId::FixedInstanceInput(fixed_id) => self.fixed_instances
                                                               .get(fixed_id)
                                                               .ok_or_else(|| anyhow!("Fixed {fixed_id} not found"))?
                                                               .get_send_state_chunk(self)?,
            SessionFlowId::FixedInstanceOutput(fixed_id) => self.fixed_instances
                                                                .get(fixed_id)
                                                                .ok_or_else(|| anyhow!("Fixed {fixed_id} not found"))?
                                                                .get_return_state_chunk(self)?,
            SessionFlowId::DynamicInstanceInput(_) => {
                return Err(anyhow!("Dynamic instances not supported yet"));
            }
            SessionFlowId::DynamicInstanceOutput(_) => {
                return Err(anyhow!("Dynamic instances not supported yet"));
            }
            SessionFlowId::TrackOutput(track_id) => self.tracks
                                                        .get(track_id)
                                                        .ok_or_else(|| anyhow!("Track {track_id} not found"))?
                                                        .get_state_chunk(self)?,
        };

        self.set_track_state_chunk(flow_id, chunk)?;

        Ok(())
    }

    fn modify_spec_one(&mut self,
                       item: ModifySessionSpec,
                       instances: &HashMap<FixedInstanceId, InstanceRouting>,
                       media: &HashMap<AppMediaObjectId, String>,
                       dirty: &mut HashSet<SessionFlowId>)
                       -> anyhow::Result<()> {
        match item {
            ModifySessionSpec::AddTrack { track_id, channels } => {
                self.add_track(track_id.clone(),
                               SessionTrack { channels,
                                              media: HashMap::new() },
                               media)?;

                dirty.insert(SessionFlowId::TrackOutput(track_id));
            }
            ModifySessionSpec::AddTrackMedia { track_id, spec } => {
                if let Some(track) = self.tracks.get_mut(&track_id) {
                    if track.add_media(media_id, spec, media)? {
                        dirty.insert(track.get_flow_id().clone());
                    }
                } else {
                    return Err(anyhow!("track {track_id} not found"));
                }
            }
            ModifySessionSpec::SetTrackMediaValues { track_id,
                                                     media_id,
                                                     channels,
                                                     media_segment,
                                                     timeline_segment,
                                                     object_id, } => {
                if let Some(track) = self.tracks.get_mut(&track_id) {
                    if track.set_media_values(media_id, channels, media_segment, timeline_segment, object_id, media)? {
                        dirty.insert(track.get_flow_id().clone());
                    }
                } else {
                    return Err(anyhow!("track {track_id} not found"));
                }
            }
            ModifySessionSpec::DeleteTrackMedia { track_id, media_id } => {
                if let Some(track) = self.tracks.get_mut(&track_id) {
                    if track.delete_media(&media_id)? {
                        dirty.insert(track.get_flow_id().clone());
                    }
                } else {
                    return Err(anyhow!("track {track_id} not found"));
                }
            }
            ModifySessionSpec::DeleteTrack { track_id } => {
                self.delete_track(&track_id);
            }
            ModifySessionSpec::AddFixedInstance { fixed_id, process } => {
                self.add_fixed_instance(fixed_id.clone(), process, instances)?;

                dirty.insert(SessionFlowId::FixedInstanceInput(fixed_id.clone()));
                dirty.insert(SessionFlowId::FixedInstanceOutput(fixed_id.clone()));
            }
            ModifySessionSpec::AddDynamicInstance { .. } => {
                // not supported, silently ignore
            }
            ModifySessionSpec::AddMixer { mixer_id, mixer } => {
                self.add_mixer(mixer_id.clone(), mixer)?;

                dirty.insert(SessionFlowId::MixerInput(mixer_id.clone()));
                dirty.insert(SessionFlowId::MixerOutput(mixer_id.clone()));
            }
            ModifySessionSpec::DeleteMixer { mixer_id } => {
                self.delete_mixer(&mixer_id);
            }
            ModifySessionSpec::DeleteFixedInstance { .. } => {}
            ModifySessionSpec::DeleteDynamicInstance { .. } => {}
            ModifySessionSpec::DeleteConnection { .. } => {}
            ModifySessionSpec::AddConnection { .. } => {}
            ModifySessionSpec::SetConnectionParameterValues { .. } => {}
            ModifySessionSpec::SetFixedInstanceParameterValues { .. } => {}
            ModifySessionSpec::SetDynamicInstanceParameterValues { .. } => {}
        }
        Ok(())
    }

    pub fn clear(&mut self) {
        let reaper = Reaper::get();

        while let Some(track) = reaper.get_track(self.context(), 0) {
            reaper.delete_track(track);
        }

        self.tracks.clear();
        self.fixed_instances.clear();
        self.mixers.clear();
    }

    pub fn on_media_updated(&mut self,
                            available: &HashMap<AppMediaObjectId, String>,
                            removed: &HashSet<AppMediaObjectId>)
                            -> anyhow::Result<()> {
        for track in self.tracks.values_mut() {
            if track.on_media_updated(available, removed) {
                track.update_state_chunk(self)?;
            }
        }

        Ok(())
    }

    pub fn on_instances_updated(&mut self,
                                instances: &HashMap<FixedInstanceId, InstanceRouting>)
                                -> anyhow::Result<()> {
        for fixed_instance in self.fixed_instances.values_mut() {
            if fixed_instance.on_instances_updated(instances) {
                fixed_instance.update_state_chunk(self)?;
            }
        }

        Ok(())
    }

    pub fn set_track_state_chunk(&self, flow_id: &SessionFlowId, chunk: String) -> anyhow::Result<()> {
        let reaper = Reaper::get();
        let index = self.track_index(flow_id.clone())
                        .ok_or_else(|| anyhow!("Track not found"))?;

        let track = reaper.get_track(self.context(), index as u32)
                          .ok_or_else(|| anyhow!("Track could not be loaded"))?;

        reaper.set_track_state_chunk(track, chunk.as_str(), ChunkCacheHint::NormalMode);

        Ok(())
    }
}
