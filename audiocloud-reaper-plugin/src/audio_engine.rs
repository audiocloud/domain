use std::collections::HashMap;
use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;

use anyhow::anyhow;
use askama::Template;
use flume::{Receiver, Sender};
use once_cell::sync::OnceCell;
use reaper_medium::{ChunkCacheHint, ControlSurface, MediaTrack, ProjectContext, Reaper, TrackDefaultsBehavior};
use serde::{Deserialize, Serialize};
use tracing::*;
use uuid::Uuid;

use audiocloud_api::audio_engine::{AudioEngineCommand, AudioEngineEvent, CompressedAudio};
use audiocloud_api::change::{PlayId, PlaySession, RenderId};
use audiocloud_api::cloud::apps::SessionSpec;
use audiocloud_api::cloud::domains::InstanceRouting;
use audiocloud_api::model::MultiChannelValue;
use audiocloud_api::newtypes::{AppMediaObjectId, AppSessionId, ConnectionId, FixedInstanceId};
use audiocloud_api::session::{MixerChannels, SessionConnection, SessionFlowId};
use project::AudioEngineProject;

use crate::audio_engine::project::AudioEngineProjectTemplateSnapshot;
use crate::events::AudioEngineCommandWithResultSender;

mod fixed_instance;
mod media_item;
mod media_track;
mod mixer;
mod project;
mod rest_api;

pub struct PluginRegistry {
    pub tx_engine: Sender<ReaperEngineCommand>,
    pub plugins:   HashMap<AppSessionId, Sender<StreamingPluginCommand>>,
}

impl PluginRegistry {
    pub fn register(id: AppSessionId, sender: Sender<StreamingPluginCommand>) -> Sender<ReaperEngineCommand> {
        let mut lock = PLUGIN_REGISTRY.get()
                                      .expect("Plugin registry exists")
                                      .lock()
                                      .expect("Plugin registry lock");
        lock.plugins.insert(id, sender);
        lock.tx_engine.clone()
    }

    pub fn unregister(id: &AppSessionId) {
        let mut lock = PLUGIN_REGISTRY.get()
                                      .expect("Plugin registry exists")
                                      .lock()
                                      .expect("Plugin registry lock");
        lock.plugins.remove(id);
    }

    pub fn play(app_session_id: &AppSessionId, play: PlaySession, context: ProjectContext) -> anyhow::Result<()> {
        let lock = PLUGIN_REGISTRY.get()
                                  .ok_or_else(|| anyhow!("failed to obtain plugin registry: not initialized?"))?
                                  .lock()
                                  .map_err(|_| anyhow!("failed to lock plugin registry"))?;

        let plugin = lock.plugins
                         .get(app_session_id)
                         .ok_or_else(|| anyhow!("No plugin for session {app_session_id}"))?;

        let _ = plugin.try_send(StreamingPluginCommand::Play { context: ProjectContext::CurrentProject,
                                                               play });

        Ok(())
    }

    pub fn flush(app_session_id: &AppSessionId, play_id: PlayId) -> anyhow::Result<()> {
        let lock = PLUGIN_REGISTRY.get()
                                  .ok_or_else(|| anyhow!("failed to obtain plugin registry: not initialized?"))?
                                  .lock()
                                  .map_err(|_| anyhow!("failed to lock plugin registry"))?;

        let plugin = lock.plugins
                         .get(app_session_id)
                         .ok_or_else(|| anyhow!("No plugin for session {app_session_id}"))?;

        let _ = plugin.try_send(StreamingPluginCommand::Flush { play_id });

        Ok(())
    }

    pub fn has(app_session_id: &AppSessionId) -> anyhow::Result<bool> {
        let lock = PLUGIN_REGISTRY.get()
                                  .ok_or_else(|| anyhow!("failed to obtain plugin registry: not initialized?"))?
                                  .lock()
                                  .map_err(|_| anyhow!("failed to lock plugin registry"))?;

        Ok(lock.plugins.contains_key(app_session_id))
    }

    pub(crate) fn init(tx_engine: Sender<ReaperEngineCommand>) {
        PLUGIN_REGISTRY.set(Mutex::new(PluginRegistry { tx_engine,
                                                        plugins: HashMap::new() }))
                       .map_err(|_| anyhow!("Plugin registry already initialized"))
                       .expect("init Plugin Registry");
    }
}

static PLUGIN_REGISTRY: OnceCell<Mutex<PluginRegistry>> = OnceCell::new();

#[derive(Debug)]
pub enum ReaperEngineCommand {
    PlayReady(AppSessionId, PlayId),
    PlayError(AppSessionId, String),
    Audio(AppSessionId, PlayId, CompressedAudio),
    Request(AudioEngineCommandWithResultSender),
    GetStatus(Sender<anyhow::Result<HashMap<AppSessionId, EngineStatus>>>),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EngineStatus {
    pub is_playing:           Option<PlayId>,
    pub is_rendering:         Option<RenderId>,
    pub is_transport_playing: bool,
    pub position:             f64,
    pub plugin_ready:         bool,
}

#[derive(Debug)]
pub enum StreamingPluginCommand {
    Play {
        context: ProjectContext,
        play:    PlaySession,
    },
    Flush {
        play_id: PlayId,
    },
}

#[derive(Debug)]
pub struct ReaperAudioEngine {
    shared_media_root: PathBuf,
    sessions:          HashMap<AppSessionId, AudioEngineProject>,
    rx_cmd:            Receiver<ReaperEngineCommand>,
    tx_evt:            Sender<AudioEngineEvent>,
}

impl Drop for ReaperAudioEngine {
    fn drop(&mut self) {
        warn!("ReaperAudioEngine dropped!");
    }
}

impl ReaperAudioEngine {
    pub(crate) fn get_status(&self) -> anyhow::Result<HashMap<AppSessionId, EngineStatus>> {
        let mut rv = HashMap::new();
        for (id, session) in &self.sessions {
            rv.insert(id.clone(), session.get_status()?);
        }

        Ok(rv)
    }
}

impl ReaperAudioEngine {
    #[instrument(skip_all)]
    pub fn new(shared_media_root: PathBuf,
               tx_cmd: Sender<ReaperEngineCommand>,
               rx_cmd: Receiver<ReaperEngineCommand>,
               tx_evt: Sender<AudioEngineEvent>)
               -> ReaperAudioEngine {
        thread::spawn(move || rest_api::run(tx_cmd));

        ReaperAudioEngine { sessions: HashMap::new(),
                            shared_media_root,
                            rx_cmd,
                            tx_evt }
    }

    #[instrument(skip_all, err)]
    fn dispatch_cmd(&mut self, cmd: AudioEngineCommand) -> anyhow::Result<()> {
        debug!(?cmd, "entered");
        match cmd {
            AudioEngineCommand::SetSpec { session_id,
                                          spec,
                                          instances,
                                          media_ready, } => {
                if let Some(project) = self.sessions.get_mut(&session_id) {
                    project.set_spec(spec, instances, media_ready)?;
                } else {
                    self.create_session(session_id, spec, instances, media_ready)?;
                }
            }
            AudioEngineCommand::Media { ready, removed } => {
                for session in self.sessions.values_mut() {
                    session.on_media_updated(&ready, &removed)?;
                }
            }
            AudioEngineCommand::ModifySpec { session_id,
                                             transaction,
                                             instances,
                                             media_ready, } => {
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    session.modify_spec(transaction, instances, media_ready)?;
                } else {
                    return Err(anyhow!("Session not found"));
                }
            }
            AudioEngineCommand::SetDynamicParameters { session_id, .. } => {
                if let Some(_) = self.sessions.get_mut(&session_id) {
                    // TODO: implement dynamic parameters
                } else {
                    return Err(anyhow!("Session not found"));
                }
            }
            AudioEngineCommand::Render { session_id, render } => {
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    session.render(render)?;
                } else {
                    return Err(anyhow!("Session not found"));
                }
            }
            AudioEngineCommand::Play { session_id, play } => {
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    session.play(play)?;
                } else {
                    return Err(anyhow!("Session not found"));
                }
            }
            AudioEngineCommand::UpdatePlay { session_id, update } => {
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    session.update_play(update)?;
                } else {
                    return Err(anyhow!("Session not found"));
                }
            }
            AudioEngineCommand::Stop { session_id } => {
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    session.stop()?;
                } else {
                    return Err(anyhow!("Session not found"));
                }
            }
            AudioEngineCommand::Instances { instances } => {
                for session in self.sessions.values_mut() {
                    session.on_instances_updated(&instances)?;
                }
            }
            AudioEngineCommand::Close { session_id } => {
                if let Some(session) = self.sessions.remove(&session_id) {
                    drop(session);
                } else {
                    return Err(anyhow!("Session not found"));
                }
            }
        }
        Ok(())
    }

    fn create_session(&mut self,
                      session_id: AppSessionId,
                      spec: SessionSpec,
                      instances: HashMap<FixedInstanceId, InstanceRouting>,
                      media: HashMap<AppMediaObjectId, String>)
                      -> anyhow::Result<()> {
        // create a temporary folder
        // within it, create a file with {app_session_id}.rpp

        let temp_dir = tempdir::TempDir::new("audiocloud-session")?;

        self.sessions.insert(session_id.clone(),
                             AudioEngineProject::new(session_id,
                                                     temp_dir,
                                                     self.shared_media_root.clone(),
                                                     spec,
                                                     instances,
                                                     media)?);

        Ok(())
    }

    pub fn send_playing_audio_event(&mut self,
                                    session_id: AppSessionId,
                                    play_id: PlayId,
                                    audio: CompressedAudio,
                                    peak_meters: HashMap<SessionFlowId, MultiChannelValue>) {
        let dynamic_reports = Default::default();
        let event = AudioEngineEvent::Playing { session_id,
                                                play_id,
                                                audio,
                                                peak_meters,
                                                dynamic_reports };

        let _ = self.tx_evt.try_send(event);
    }
}

impl ControlSurface for ReaperAudioEngine {
    #[instrument(skip(self))]
    fn run(&mut self) {
        while let Ok(cmd) = self.rx_cmd.try_recv() {
            match cmd {
                ReaperEngineCommand::Audio(session_id, play_id, audio) => {
                    if let Some(session) = self.sessions.get(&session_id) {
                        self.send_playing_audio_event(session_id, play_id, audio, session.get_peak_meters());
                    } else {
                        warn!(%session_id, "Session not found");
                    }
                }
                ReaperEngineCommand::Request((cmd, sender)) => {
                    if let Err(err) = sender.send(self.dispatch_cmd(cmd)) {
                        warn!(%err, "failed to send response to command");
                    }
                }
                ReaperEngineCommand::PlayReady(session_id, play_id) => {
                    if let Some(session) = self.sessions.get_mut(&session_id) {
                        session.play_ready(play_id);
                    } else {
                        warn!(%session_id, "Session not found");
                    }
                }
                ReaperEngineCommand::GetStatus(send_status) => {
                    let _ = send_status.send(self.get_status());
                }
                ReaperEngineCommand::PlayError(session_id, error) => {
                    let _ = self.tx_evt.send(AudioEngineEvent::Error { session_id, error });
                }
            }
        }

        for (_, session) in &mut self.sessions {
            let _ = session.run();
            while let Some(event) = session.events.pop_front() {
                debug!(?event, "emitting");
                let _ = self.tx_evt.try_send(event);
            }
        }
    }
}

pub fn beautify_chunk(chunk: String) -> String {
    let mut tab = 0;

    chunk.lines()
         .flat_map(|line| {
             let line = line.trim();
             let mut this_line_tab = tab;
             if line.starts_with('<') {
                 tab += 1;
             } else if line.ends_with('>') {
                 tab -= 1;
                 this_line_tab -= 1;
             }

             if line.is_empty() {
                 None
             } else {
                 Some("\t".repeat(this_line_tab) + line)
             }
         })
         .collect::<Vec<_>>()
         .join("\n")
}

#[derive(Template)]
#[template(path = "audio_engine/auxrecv_connection.txt")]
struct ConnectionTemplate<'a> {
    id:         &'a ConnectionId,
    project:    &'a AudioEngineProjectTemplateSnapshot,
    connection: &'a SessionConnection,
}

impl<'a> ConnectionTemplate<'a> {
    pub fn new(project: &'a AudioEngineProjectTemplateSnapshot,
               id: &'a ConnectionId,
               connection: &'a SessionConnection)
               -> Self {
        Self { id,
               project,
               connection }
    }

    fn source_reaper_channel(&self) -> i32 {
        match self.connection.from_channels {
            MixerChannels::Mono(start) => (start | 1024) as i32,
            MixerChannels::Stereo(start) => start as i32,
        }
    }

    fn dest_reaper_channel(&self) -> i32 {
        match self.connection.to_channels {
            MixerChannels::Mono(start) => (start | 1024) as i32,
            MixerChannels::Stereo(start) => start as i32,
        }
    }
}

pub(crate) fn get_track_uuid(track: MediaTrack) -> Uuid {
    let reaper = Reaper::get();
    let uuid = unsafe { reaper.get_set_media_track_info_get_guid(track) };
    let cstr = reaper.guid_to_string(&uuid);
    let s = cstr.to_str();
    Uuid::try_parse(&s[1..s.len() - 1]).unwrap_or_else(|_| Uuid::new_v4())
}

pub(crate) fn append_track(flow_id: &SessionFlowId, context: ProjectContext) -> anyhow::Result<(MediaTrack, Uuid)> {
    let reaper = Reaper::get();

    let index = reaper.count_tracks(context);

    reaper.insert_track_at_index(index, TrackDefaultsBehavior::OmitDefaultEnvAndFx);

    let track = reaper.get_track(context, index)
                      .ok_or_else(|| anyhow!("failed to get track we just created"))?;

    unsafe {
        reaper.get_set_media_track_info_set_name(track, flow_id.to_string().as_str());
    }

    let track_id = get_track_uuid(track);

    Ok((track, track_id))
}

#[instrument(skip_all)]
pub(crate) fn delete_track(context: ProjectContext, track: MediaTrack) {
    let reaper = Reaper::get();
    if reaper.validate_ptr_2(context, track) {
        unsafe {
            reaper.delete_track(track);
        }
        debug!("deleted");
    } else {
        warn!(?track, "invalid track");
    }
}

#[instrument(skip_all, err)]
pub(crate) fn set_track_chunk(context: ProjectContext, track: MediaTrack, chunk: &str) -> anyhow::Result<()> {
    let reaper = Reaper::get();
    unsafe {
        if reaper.validate_ptr_2(context, track) {
            reaper.set_track_state_chunk(track, chunk, ChunkCacheHint::NormalMode)?;
            debug!(?chunk, "chunk set");
        } else {
            warn!(?track, "invalid track");
        }
    }

    Ok(())
}
