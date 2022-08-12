use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{CStr, CString};
use std::fs;
use std::path::PathBuf;
use std::ptr::null_mut;

use anyhow::anyhow;
use askama::Template;
use cstr::cstr;
use lazy_static::lazy_static;
use reaper_medium::ProjectContext::CurrentProject;
use reaper_medium::{
    AutoSeekBehavior, ChunkCacheHint, CommandId, EditMode, MediaTrack, PlayState, PositionInSeconds, ProjectContext,
    ProjectRef, ReaProject, Reaper, ReaperPanValue, ReaperVolumeValue, SetEditCurPosOptions, TimeRangeType,
    TrackAttributeKey, TrackSendCategory, TrackSendRef,
};
use tempdir::TempDir;
use tracing::*;

use audiocloud_api::audio_engine::AudioEngineEvent;
use audiocloud_api::change::{ModifySessionSpec, PlayId, PlaySession, RenderId, RenderSession, UpdatePlaySession};
use audiocloud_api::cloud::apps::SessionSpec;
use audiocloud_api::cloud::domains::InstanceRouting;
use audiocloud_api::model::{ModelValue, MultiChannelValue};
use audiocloud_api::newtypes::{
    AppMediaObjectId, AppSessionId, ConnectionId, FixedId, FixedInstanceId, MixerId, TrackId,
};
use audiocloud_api::session::{
    ConnectionValues, SessionConnection, SessionFixedInstance, SessionFlowId, SessionMixer, SessionTimeSegment,
    SessionTrack,
};
use audiocloud_api::time::Timestamped;

use crate::audio_engine::fixed_instance::AudioEngineFixedInstance;
use crate::audio_engine::media_track::AudioEngineMediaTrack;
use crate::audio_engine::mixer::AudioEngineMixer;
use crate::audio_engine::{EngineStatus, PluginRegistry};

#[derive(Debug, Clone)]
pub enum ProjectPlayState {
    PreparingToPlay(PlaySession),
    Playing(PlaySession),
    Rendering(RenderSession),
    Stopped,
}

#[derive(Debug)]
pub struct AudioEngineProject {
    id:                    AppSessionId,
    project:               ReaProject,
    tracks:                HashMap<TrackId, AudioEngineMediaTrack>,
    fixed_instances:       HashMap<FixedId, AudioEngineFixedInstance>,
    mixers:                HashMap<MixerId, AudioEngineMixer>,
    spec:                  SessionSpec,
    local_media_root:      PathBuf,
    shared_media_root:     PathBuf,
    pub play_state:        Timestamped<ProjectPlayState>,
    pub temp_dir:          TempDir,
    pub session_path:      PathBuf,
    pub reaper_play_state: Timestamped<PlayState>,
    pub events:            VecDeque<AudioEngineEvent>,
}

#[derive(Debug, Clone)]
pub struct AudioEngineProjectTemplateSnapshot {
    context:     ProjectContext,
    connections: HashMap<ConnectionId, SessionConnection>,
}

impl AudioEngineProjectTemplateSnapshot {
    pub fn context(&self) -> ProjectContext {
        self.context
    }

    pub fn track_index(&self, flow_id: SessionFlowId) -> Option<usize> {
        let reaper = Reaper::get();
        let mut index = 0;
        let flow_name = flow_id.to_string();

        while let Some(track) = reaper.get_track(self.context, index) {
            let matches =
                unsafe { reaper.get_set_media_track_info_get_name(track, |name| name.to_str() == flow_name.as_str()) };

            if matches.unwrap_or(false) {
                return Some(index as usize);
            }

            index += 1;
        }

        None
    }

    pub fn fixed_input_track_index(&self, fixed_id: &FixedId) -> Option<usize> {
        self.track_index(SessionFlowId::FixedInstanceInput(fixed_id.clone()))
    }

    pub fn mixer_input_track_index(&self, mixer_id: &MixerId) -> Option<usize> {
        self.track_index(SessionFlowId::MixerInput(mixer_id.clone()))
    }

    pub fn flows_to<'a>(&'a self,
                        flow: &'a SessionFlowId)
                        -> impl Iterator<Item = (&ConnectionId, &SessionConnection)> + 'a {
        self.connections.iter().filter(move |(_, conn)| &conn.to == flow)
    }
}

lazy_static! {
    static ref CMD_REC_MODE_SET_TIME_RANGE_AUTO_PUNCH: CommandId = CommandId::new(40076);
    static ref CMD_CREATE_PROJECT_TAB: CommandId = CommandId::new(40859);
    static ref CMD_CLOSE_CURRENT_PROJECT_TAB: CommandId = CommandId::new(40860);
    static ref CMD_SWITCH_TO_NEXT_PROJECT_TAB: CommandId = CommandId::new(40861);
    static ref CMD_TRANSPORT_RECORD: CommandId = CommandId::new(1013);
    static ref CMD_TRANSPORT_STOP_AND_SAVE_MEDIA: CommandId = CommandId::new(40667);
    static ref CMD_TRANSPORT_STOP_AND_DELETE_MEDIA: CommandId = CommandId::new(40668);
}

#[derive(Template)]
#[template(path = "audio_engine/project.txt")]
struct AudioEngineProjectTemplate<'a> {
    spec:       &'a SessionSpec,
    session_id: &'a AppSessionId,
    media_root: String,
}

impl AudioEngineProject {
    #[instrument(skip_all, err)]
    pub fn new(id: AppSessionId,
               temp_dir: TempDir,
               shared_media_root: PathBuf,
               session_spec: SessionSpec,
               instances: HashMap<FixedInstanceId, InstanceRouting>,
               media: HashMap<AppMediaObjectId, String>)
               -> anyhow::Result<Self> {
        let reaper = Reaper::get();

        // this is not OK we need to configure it
        let local_media_root = temp_dir.path().join("media");
        let session_path = temp_dir.path().join("session.rpp");

        fs::write(&session_path,
                  AudioEngineProjectTemplate { spec:       &session_spec,
                                               session_id: &id,
                                               media_root: local_media_root.to_string_lossy().to_string(), }.render()?)?;

        reaper.main_on_command_ex(*CMD_CREATE_PROJECT_TAB, 0, CurrentProject);

        unsafe {
            let path_as_cstr = CString::new(format!("noprompt:{}", session_path.to_string_lossy()))?;
            reaper.low().Main_openProject(path_as_cstr.as_ptr());
        }

        let project = reaper.enum_projects(ProjectRef::Current, 0)
                            .ok_or_else(|| anyhow!("No current project even though we just opened one"))?
                            .project;

        let context = ProjectContext::Proj(project);

        reaper.main_on_command_ex(*CMD_REC_MODE_SET_TIME_RANGE_AUTO_PUNCH, 0, context);

        let tracks = Default::default();
        let fixed_instances = Default::default();
        let mixers = Default::default();
        let spec = Default::default();
        let play_state = ProjectPlayState::Stopped.into();
        let reaper_play_state = Timestamped::from(Reaper::get().get_play_state_ex(context));
        let events = VecDeque::new();

        let mut rv = Self { id,
                            project,
                            tracks,
                            fixed_instances,
                            mixers,
                            spec,
                            local_media_root,
                            shared_media_root,
                            temp_dir,
                            session_path,
                            play_state,
                            reaper_play_state,
                            events };

        rv.set_spec(session_spec, instances, media)?;

        Ok(rv)
    }

    pub fn template_snapshot(&self) -> AudioEngineProjectTemplateSnapshot {
        AudioEngineProjectTemplateSnapshot { context:     self.context(),
                                             connections: self.spec.connections.clone(), }
    }

    pub fn play_ready(&mut self, play_id: PlayId) {
        if let ProjectPlayState::PreparingToPlay(play) = self.play_state.value() {
            if play.play_id == play_id {
                self.play_state = ProjectPlayState::Playing(play.clone()).into();
            }
        }
    }

    #[instrument(skip_all, err, fields(id = %self.id))]
    pub fn run(&mut self) -> anyhow::Result<()> {
        let context = self.context();

        if let ProjectPlayState::PreparingToPlay(_) = self.play_state.value() {
            if self.play_state.elapsed().num_seconds() > 1 {
                self.play_state = ProjectPlayState::Stopped.into();
                self.events.push_back(AudioEngineEvent::Error { session_id: self.id.clone(),
                                                     error:
                                                         format!("Timed out preparing resampling or compression"), });
            }
        }

        let reaper = Reaper::get();
        let new_play_state = reaper.get_play_state_ex(context);
        let cur_pos = reaper.get_play_position_ex(context).get();

        if &new_play_state != self.reaper_play_state.value() {
            match self.play_state.value().clone() {
                ProjectPlayState::Playing(play) if !new_play_state.is_playing => {
                    self.clean_up_end_of_play(play.play_id);
                }
                ProjectPlayState::Rendering(render) if render.segment.end() >= cur_pos => {
                    self.clean_up_end_of_render(render.mixer_id.clone(), render.render_id);
                }
                _ => {}
            }
        }

        self.reaper_play_state = Timestamped::from(new_play_state);

        Ok(())
    }

    fn clean_up_end_of_render(&mut self, mixer_id: MixerId, render_id: RenderId) {
        let reaper = Reaper::get();
        let context = self.context();

        reaper.main_on_command_ex(*CMD_TRANSPORT_STOP_AND_SAVE_MEDIA, 0, context);

        if let Some(mixer) = self.mixers.get_mut(&mixer_id) {
            if let Some(path) = mixer.clear_render() {
                self.events
                    .push_back(AudioEngineEvent::RenderingFinished { session_id: self.id.clone(),
                                                                     render_id,
                                                                     path });
            } else {
                // we did not get a path
                self.events
                    .push_back(AudioEngineEvent::RenderingFailed { session_id: self.id.clone(),
                                                                   render_id,
                                                                   reason: format!("Rendered file not found") });
            }

            self.play_state = ProjectPlayState::Stopped.into();
        }
    }

    fn clean_up_end_of_play(&mut self, play_id: PlayId) {
        self.clear_mixer_master_sends();
        // a plugin flush is not critical, so we are fine with discarding the error
        let _ = PluginRegistry::flush(&self.id, play_id);

        self.play_state = ProjectPlayState::Stopped.into();
    }

    pub fn context(&self) -> ProjectContext {
        ProjectContext::Proj(self.project)
    }

    pub fn shared_media_root_dir(&self) -> PathBuf {
        self.shared_media_root.clone()
    }

    pub fn get_peak_meters(&self) -> HashMap<SessionFlowId, MultiChannelValue> {
        let mut rv = HashMap::new();
        for track in self.tracks.values() {
            track.fill_peak_meters(&mut rv);
        }
        for mixer in self.mixers.values() {
            mixer.fill_peak_meters(&mut rv);
        }
        for instance in self.fixed_instances.values() {
            instance.fill_peak_meters(&mut rv);
        }

        rv
    }

    #[instrument(skip_all, err)]
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
                    reaper.main_on_command_ex(*CMD_SWITCH_TO_NEXT_PROJECT_TAB, 0, CurrentProject);
                } else {
                    break;
                }
            }
        }

        Ok(())
    }
}

impl Drop for AudioEngineProject {
    fn drop(&mut self) {
        if self.project.as_ptr() != null_mut() {
            let reaper = Reaper::get();
            if let Ok(_) = self.focus() {
                debug!(id = %self.id, "Closing project");
                reaper.main_on_command_ex(*CMD_CLOSE_CURRENT_PROJECT_TAB, 0, self.context());
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

    pub fn get_status(&self) -> anyhow::Result<EngineStatus> {
        Ok(EngineStatus { plugin_ready:         PluginRegistry::has(&self.id)?,
                          is_transport_playing: self.reaper_play_state.value().is_playing
                                                || self.reaper_play_state.value().is_recording,
                          is_playing:           if let ProjectPlayState::Playing(play) = self.play_state.value() {
                              Some(play.play_id.clone())
                          } else {
                              None
                          },
                          is_rendering:         if let ProjectPlayState::Rendering(render) = self.play_state.value() {
                              Some(render.render_id.clone())
                          } else {
                              None
                          },
                          position:             Reaper::get().get_play_position_ex(self.context()).get(), })
    }

    pub fn render(&mut self, render: RenderSession) -> anyhow::Result<()> {
        let reaper = Reaper::get();

        self.stop()?;
        self.clear_mixer_master_sends();
        self.set_time_range_markers(render.segment);
        self.clear_all_project_markers();
        self.set_looping(false);
        self.set_play_position((render.segment.start - 0.125).max(0.0), false);

        if let Some(mixer) = self.mixers.get_mut(&render.mixer_id) {
            mixer.prepare_render(&render);
        }

        self.play_state = ProjectPlayState::Rendering(render).into();

        reaper.main_on_command_ex(*CMD_TRANSPORT_RECORD, 0, self.context());

        Ok(())
    }

    pub fn play(&mut self, play: PlaySession) -> anyhow::Result<()> {
        let reaper = Reaper::get();

        self.stop()?;

        for (mixer_id, mixer) in &mut self.mixers {
            mixer.set_master_send(mixer_id == &play.mixer_id);
        }

        self.clear_all_project_markers();
        self.set_time_range_markers(play.segment);
        self.set_play_position(play.start_at, false);
        self.set_looping(play.looping);

        PluginRegistry::play(&self.id, play.clone(), self.context())?;

        self.play_state = ProjectPlayState::PreparingToPlay(play).into();

        Ok(())
    }

    pub fn update_play(&mut self, update: UpdatePlaySession) -> anyhow::Result<()> {
        let reaper = Reaper::get();

        if let Some(new_mixer_id) = update.mixer_id {
            for (mixer_id, mixer) in self.mixers.iter_mut() {
                mixer.set_master_send(mixer_id == &new_mixer_id);
            }
        }

        if let Some(segment) = update.segment {
            self.set_time_range_markers(segment);
        }

        self.set_looping(update.looping);

        if let Some(start_at) = update.start_at {
            self.set_play_position(start_at, true);
        }

        Ok(())
    }

    pub fn stop(&mut self) -> anyhow::Result<()> {
        let reaper = Reaper::get();
        let context = self.context();

        match self.play_state.value() {
            ProjectPlayState::Rendering(render) => {
                // this is an incomplete render...
                reaper.main_on_command_ex(*CMD_TRANSPORT_STOP_AND_DELETE_MEDIA, 0, context);
                if let Some(mixer) = self.mixers.get_mut(&render.mixer_id) {
                    let _ = mixer.clear_render();
                }

                self.events.push_back(AudioEngineEvent::RenderingFailed {
                    session_id: self.id.clone(),
                    render_id: render.render_id,
                    reason: format!("Rendering stopped prematurely")
                });
            }
            _ => {
                reaper.on_stop_button_ex(context);
            }
        }

        self.play_state = ProjectPlayState::Stopped.into();

        Ok(())
    }

    #[instrument(skip_all, err)]
    pub fn set_spec(&mut self,
                    spec: SessionSpec,
                    instances: HashMap<FixedInstanceId, InstanceRouting>,
                    media: HashMap<AppMediaObjectId, String>)
                    -> anyhow::Result<()> {
        if &self.spec == &spec {
            debug!("incoming spec is the same, not changing anything");
            return Ok(());
        }

        debug!(?spec, "new spec");

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
        let snapshot = self.template_snapshot();

        for track in self.tracks.values() {
            track.update_state_chunk(&snapshot)?;
        }

        for instance in self.fixed_instances.values() {
            instance.update_state_chunk(&snapshot)?;
        }

        for mixer in self.mixers.values() {
            mixer.update_state_chunk(&snapshot)?;
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
        let snapshot = self.template_snapshot();
        let chunk = match flow_id {
            SessionFlowId::MixerInput(mixer_id) => self.mixers
                                                       .get(mixer_id)
                                                       .ok_or_else(|| anyhow!("Mixer {mixer_id} not found"))?
                                                       .get_input_state_chunk(&snapshot)?,
            SessionFlowId::MixerOutput(mixer_id) => self.mixers
                                                        .get(mixer_id)
                                                        .ok_or_else(|| anyhow!("Mixer {mixer_id} not found"))?
                                                        .get_output_state_chunk(&snapshot)?,
            SessionFlowId::FixedInstanceInput(fixed_id) => self.fixed_instances
                                                               .get(fixed_id)
                                                               .ok_or_else(|| anyhow!("Fixed {fixed_id} not found"))?
                                                               .get_send_state_chunk(&snapshot)?,
            SessionFlowId::FixedInstanceOutput(fixed_id) => self.fixed_instances
                                                                .get(fixed_id)
                                                                .ok_or_else(|| anyhow!("Fixed {fixed_id} not found"))?
                                                                .get_return_state_chunk(&snapshot)?,
            SessionFlowId::DynamicInstanceInput(_) => {
                return Err(anyhow!("Dynamic instances not supported yet"));
            }
            SessionFlowId::DynamicInstanceOutput(_) => {
                return Err(anyhow!("Dynamic instances not supported yet"));
            }
            SessionFlowId::TrackOutput(track_id) => self.tracks
                                                        .get(track_id)
                                                        .ok_or_else(|| anyhow!("Track {track_id} not found"))?
                                                        .get_state_chunk(&snapshot)?,
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
            ModifySessionSpec::AddTrackMedia { track_id,
                                               media_id,
                                               spec, } => {
                if let Some(track) = self.tracks.get_mut(&track_id) {
                    if track.add_media(media_id, spec, media)? {
                        dirty.insert(track.get_flow_id().clone());
                    }
                } else {
                    return Err(anyhow!("track {track_id} not found"));
                }
            }
            ModifySessionSpec::UpdateTrackMedia { track_id,
                                                  media_id,
                                                  update, } => {
                if let Some(track) = self.tracks.get_mut(&track_id) {
                    if track.set_media_values(media_id, update, media)? {
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
            ModifySessionSpec::DeleteFixedInstance { fixed_id } => {
                self.delete_fixed_instance(fixed_id)?;
            }
            ModifySessionSpec::DeleteDynamicInstance { .. } => {}
            ModifySessionSpec::DeleteConnection { connection_id } => {
                if let Some(connection) = self.spec.connections.remove(&connection_id) {
                    dirty.insert(connection.to.clone());
                }
            }
            ModifySessionSpec::AddConnection { to, .. } => {
                dirty.insert(to);
            }
            ModifySessionSpec::SetConnectionParameterValues { connection_id, values } => {
                if let Some(connection) = self.spec.connections.get(&connection_id) {
                    self.set_connection_parameter_values(&connection.to, &connection_id, values)?;
                } else {
                    return Err(anyhow!("connection {connection_id} not found"));
                }
            }
            ModifySessionSpec::SetFixedInstanceParameterValues { .. } => {}
            ModifySessionSpec::SetDynamicInstanceParameterValues { .. } => {}
        }
        Ok(())
    }

    pub fn clear(&mut self) {
        let reaper = Reaper::get();

        while let Some(track) = reaper.get_track(self.context(), 0) {
            unsafe {
                reaper.delete_track(track);
            }
        }

        self.tracks.clear();
        self.fixed_instances.clear();
        self.mixers.clear();
    }

    pub fn on_media_updated(&mut self,
                            available: &HashMap<AppMediaObjectId, String>,
                            removed: &HashSet<AppMediaObjectId>)
                            -> anyhow::Result<()> {
        let snapshot = self.template_snapshot();
        for track in self.tracks.values_mut() {
            if track.on_media_updated(available, removed) {
                track.update_state_chunk(&snapshot)?;
            }
        }

        Ok(())
    }

    pub fn on_instances_updated(&mut self,
                                instances: &HashMap<FixedInstanceId, InstanceRouting>)
                                -> anyhow::Result<()> {
        let snapshot = self.template_snapshot();

        for fixed_instance in self.fixed_instances.values_mut() {
            if fixed_instance.on_instances_updated(instances) {
                fixed_instance.update_state_chunk(&snapshot)?;
            }
        }

        Ok(())
    }

    pub fn set_track_state_chunk(&self, flow_id: &SessionFlowId, chunk: String) -> anyhow::Result<()> {
        let reaper = Reaper::get();
        let index = self.template_snapshot()
                        .track_index(flow_id.clone())
                        .ok_or_else(|| anyhow!("Track not found"))?;

        let track = reaper.get_track(self.context(), index as u32)
                          .ok_or_else(|| anyhow!("Track could not be loaded"))?;

        unsafe {
            reaper.set_track_state_chunk(track, chunk.as_str(), ChunkCacheHint::NormalMode)?;
        }

        Ok(())
    }

    fn set_connection_parameter_values(&self,
                                       target: &SessionFlowId,
                                       id: &ConnectionId,
                                       values: ConnectionValues)
                                       -> anyhow::Result<()> {
        let track =
            match target {
                SessionFlowId::MixerInput(mixer_id) => self.mixers.get(mixer_id).map(|mixer| mixer.get_input_track()),
                SessionFlowId::FixedInstanceInput(fixed_id) => {
                    self.fixed_instances
                        .get(fixed_id)
                        .map(|fixed_instance| fixed_instance.get_input_track())
                }
                other => return Err(anyhow!("Unsupported target {other}")),
            }.ok_or_else(|| anyhow!("Connection target {target} not found"))?;

        let index = get_track_receive_index(track, id).ok_or_else(|| {
                                                          anyhow!("Connection not found on target {target} input track")
                                                      })?;

        let reaper = Reaper::get();

        // TODO: if we need any dB conversions, now is a good time :)
        let index = TrackSendRef::Receive(index as u32);

        if let Some(volume) = values.volume {
            unsafe {
                reaper.set_track_send_ui_vol(track, index, ReaperVolumeValue::new(volume), EditMode::NormalTweak)?;
            }
        }

        if let Some(pan) = values.pan {
            unsafe {
                reaper.set_track_send_ui_pan(track, index, ReaperPanValue::new(pan), EditMode::NormalTweak)?;
            }
        }

        Ok(())
    }

    fn clear_mixer_master_sends(&mut self) {
        for (mixer_id, mixer) in &mut self.mixers {
            mixer.set_master_send(false);
        }
    }

    fn set_time_range_markers(&mut self, segment: SessionTimeSegment) {
        Reaper::get().get_set_loop_time_range_2_set(self.context(),
                                                    TimeRangeType::TimeSelection,
                                                    PositionInSeconds::new(segment.start),
                                                    PositionInSeconds::new(segment.end()),
                                                    AutoSeekBehavior::DenyAutoSeek);
    }

    fn set_play_position(&mut self, position: f64, and_play: bool) {
        Reaper::get().set_edit_curs_pos_2(self.context(),
                                          PositionInSeconds::new(position),
                                          SetEditCurPosOptions { seek_play: and_play,
                                                                 move_view: true, });
    }

    fn set_looping(&mut self, looping: bool) {
        Reaper::get().get_set_repeat_ex_set(self.context(), looping);
    }

    fn clear_all_project_markers(&mut self) {
        let reaper = Reaper::get();
        for _ in 0..reaper.count_project_markers(self.context()).total_count {
            unsafe {
                reaper.low().DeleteProjectMarkerByIndex(self.project.as_ptr(), 0);
            }
        }
    }
}

fn get_track_receive_index(track: MediaTrack, id: &ConnectionId) -> Option<usize> {
    const P_EXT_ID: &'static CStr = cstr!("P_EXT:ID");

    let reaper = Reaper::get();

    for i in 0..unsafe { reaper.get_track_num_sends(track, TrackSendCategory::Receive) } {
        let mut buffer = [0i8; 256];
        unsafe {
            if reaper.low().GetSetTrackSendInfo_String(track.as_ptr(),
                                                       TrackSendCategory::Receive.to_raw(),
                                                       i as i32,
                                                       P_EXT_ID.as_ptr(),
                                                       buffer.as_mut_ptr(),
                                                       false)
            {
                let ext_id = CStr::from_ptr(buffer.as_ptr()).to_string_lossy();

                if ext_id == id.as_str() {
                    return Some(i as usize);
                }
            }
        }
    }

    None
}

pub fn set_track_master_send(track: MediaTrack, mut send: bool) {
    unsafe {
        Reaper::get().get_set_media_track_info(track, TrackAttributeKey::MainSend, &mut send as *mut _ as _);
    }
}

pub fn get_track_peak_meters(track: MediaTrack, channels: usize) -> MultiChannelValue {
    let reaper = Reaper::get();
    let mut mcv = MultiChannelValue::new();
    for i in 0..channels {
        let value = 0.0f64;
        mcv.push(Some(ModelValue::Number(unsafe { reaper.track_get_peak_info(track, i as u32) }.into())));
    }

    mcv
}
