use std::collections::HashMap;
use std::fmt::Debug;

use anyhow::anyhow;
use askama::Template;
use flume::{Receiver, Sender};
use reaper_medium::{ChunkCacheHint, ControlSurface, MediaTrack, ProjectContext, Reaper, TrackDefaultsBehavior};
use tracing::warn;
use uuid::Uuid;

use audiocloud_api::audio_engine::{AudioEngineCommand, AudioEngineEvent};
use audiocloud_api::cloud::apps::SessionSpec;
use audiocloud_api::cloud::domains::InstanceRouting;
use audiocloud_api::newtypes::{AppMediaObjectId, AppSessionId, ConnectionId, FixedInstanceId};
use audiocloud_api::session::{MixerChannels, SessionConnection, SessionFlowId};
use project::AudioEngineProject;

use crate::events::AudioEngineCommandWithResultSender;

mod fixed_instance;
mod media_item;
mod media_track;
mod mixer;
mod project;

#[derive(Debug)]
pub struct ReaperAudioEngine {
    sessions: HashMap<AppSessionId, AudioEngineProject>,
    rx_cmd:   Receiver<AudioEngineCommandWithResultSender>,
    tx_evt:   Sender<AudioEngineEvent>,
}

impl ReaperAudioEngine {
    pub fn new(rx_cmd: Receiver<AudioEngineCommandWithResultSender>,
               tx_evt: Sender<AudioEngineEvent>)
               -> ReaperAudioEngine {
        ReaperAudioEngine { sessions: HashMap::new(),
                            rx_cmd,
                            tx_evt }
    }

    fn dispatch_cmd(&mut self, cmd: AudioEngineCommand) -> anyhow::Result<()> {
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
                    session.delete()?;
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
                             AudioEngineProject::new(session_id, temp_dir, spec, instances, media)?);

        Ok(())
    }
}

impl ControlSurface for ReaperAudioEngine {
    fn run(&mut self) {
        // TODO: dispatch commands received
        while let Ok((cmd, sender)) = self.rx_cmd.try_recv() {
            if let Err(err) = sender.send(self.dispatch_cmd(cmd)) {
                warn!(%err, "failed to send response to command");
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
    project:    &'a AudioEngineProject,
    connection: &'a SessionConnection,
}

impl<'a> ConnectionTemplate<'a> {
    pub fn new(project: &'a AudioEngineProject, id: &'a ConnectionId, connection: &'a SessionConnection) -> Self {
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
    let uuid = reaper.get_set_media_track_info_get_guid(track);
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

    reaper.get_set_media_track_info_set_name(track, flow_id.to_string().as_str());

    let track_id = get_track_uuid(track);

    Ok((track, track_id))
}

pub(crate) fn delete_track(context: ProjectContext, track: MediaTrack) {
    let reaper = Reaper::get();
    if reaper.validate_ptr_2(context, track) {
        unsafe {
            reaper.delete_track(track);
        }
    }
}

pub(crate) fn set_track_chunk(track: MediaTrack, chunk: &str) -> anyhow::Result<()> {
    let reaper = Reaper::get();
    reaper.set_track_state_chunk(track, chunk, ChunkCacheHint::NormalMode)?;

    Ok(())
}
