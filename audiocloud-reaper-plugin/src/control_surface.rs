use anyhow::anyhow;
use audiocloud_api::change::{ModifySession, ModifySessionSpec, PlaySession, RenderSession};
use flume::{Receiver, Sender};
use maplit::hashmap;

use std::collections::HashMap;
use std::fmt::Display;
use std::mem;
use std::str::FromStr;
use tracing::*;

use audiocloud_api::model::{ModelValue, MultiChannelTimestampedValue, MultiChannelValue};
use audiocloud_api::newtypes::{AppSessionId, DynamicId, ParameterId, TrackId};
use audiocloud_api::session::SessionObjectId;
use audiocloud_api::time::Timestamped;
use reaper_medium::ProjectContext::CurrentProject;
use reaper_medium::{ControlSurface, PlayState, Reaper, TrackAttributeKey};

use audiocloud_api::audio_engine::{AudioEngineCommand, AudioEngineEvent};
use audiocloud_api::cloud::apps::SessionSpec;

pub mod track;

#[derive(Debug)]
pub struct AudiocloudControlSurface {
    rx_cmd:        flume::Receiver<AudioEngineCommand>,
    tx_evt:        flume::Sender<AudioEngineEvent>,
    tx_mtr:        flume::Sender<HashMap<ReaperTrackId, MultiChannelTimestampedValue>>,
    spec:          SessionSpec,
    mode:          EngineMode,
    reaper_tracks: Vec<ReaperTrackId>,
    tracks:        HashMap<TrackId, track::ReaperTrack>,
    play_state:    PlayState,
    session_id:    AppSessionId,
}

#[derive(Debug)]
pub enum EngineMode {
    Stopped,
    Rendering(RenderSession),
    Playing(PlaySession),
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReaperTrackId {
    Input(SessionObjectId),
    Output(SessionObjectId),
}

impl Display for ReaperTrackId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReaperTrackId::Input(id) => write!(f, "inp:{id}"),
            ReaperTrackId::Output(id) => write!(f, "out:{id}"),
        }
    }
}

impl FromStr for ReaperTrackId {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let index = s.find(':').ok_or_else(|| anyhow!("could not find delimiter"))?;
        let remainder = &s[index + 1..];

        Ok(match &s[..index] {
            "inp" => ReaperTrackId::Input(SessionObjectId::from_str(remainder)?),
            "out" => ReaperTrackId::Output(SessionObjectId::from_str(remainder)?),
            tag => return Err(anyhow!("unknown tag {tag}")),
        })
    }
}

impl ControlSurface for AudiocloudControlSurface {
    fn run(&mut self) {
        while let Ok(msg) = self.rx_cmd.try_recv() {
            match msg {
                AudioEngineCommand::SetSpec { session_id, spec } => self.set_spec(spec),
                AudioEngineCommand::ModifySpec { session_id,
                                                 transaction, } => self.modify_spec(transaction),
                AudioEngineCommand::SetDynamicParameters { session_id,
                                                           dynamic_id,
                                                           parameters, } => {
                    self.set_dynamic_parameters(dynamic_id, parameters)
                }
                AudioEngineCommand::Render { session_id, render } => self.start_render(render),
                AudioEngineCommand::Play { session_id, play } => self.start_play(play),
                AudioEngineCommand::Stop { session_id } => self.stop(),
            }
        }

        // take measurements from peak meters and send to integration task
        let metering = self.get_track_metering();

        // send all the metering
        if let Err(err) = self.tx_mtr.try_send(metering) {
            warn!(%err, "Could not deliver metering");
        }

        // update current play state, update mode, emit events if needed
        self.update_play_state();
    }
}

impl AudiocloudControlSurface {
    pub fn new(session_id: AppSessionId,
               spec: SessionSpec,
               rx_cmd: Receiver<AudioEngineCommand>,
               tx_evt: Sender<AudioEngineEvent>,
               tx_mtr: Sender<HashMap<ReaperTrackId, MultiChannelTimestampedValue>>)
               -> Self {
        let play_state = Reaper::get().get_play_state_ex(CurrentProject);
        let rv = Self { rx_cmd,
                        tx_evt,
                        tx_mtr,
                        spec,
                        mode: EngineMode::Stopped,
                        reaper_tracks: vec![],
                        tracks: hashmap! {},
                        play_state,
                        session_id };

        rv
    }

    fn set_spec(&mut self, spec: SessionSpec) {
        self.spec = spec;
    }

    fn modify_spec(&mut self, transaction: Vec<ModifySessionSpec>) {}

    fn set_dynamic_parameters(&self, dynamic_id: DynamicId, parameters: HashMap<ParameterId, MultiChannelValue>) {}

    fn start_render(&mut self, render: RenderSession) {
        self.mode = EngineMode::Rendering(render);
    }

    fn start_play(&mut self, play: PlaySession) {
        self.mode = EngineMode::Playing(play);
    }

    fn stop(&mut self) {
        Reaper::get().on_stop_button_ex(CurrentProject);
    }

    fn update_play_state(&mut self) {
        let reaper = Reaper::get();
        let new_play_state = reaper.get_play_state_ex(CurrentProject);
        let stopped_after_play = !self.play_state.is_playing && new_play_state.is_playing;
        let current_pos = reaper.get_play_position_2_ex(CurrentProject).get();

        match &self.mode {
            EngineMode::Rendering(render) => {
                if stopped_after_play {
                    let success = current_pos >= render.segment.end() * 0.98;
                    let track_id = ReaperTrackId::Output(SessionObjectId::Mixer(render.mixer_id.clone()));
                    let path = self.clear_track_media_items(&track_id);
                    if let Some(path) = path {
                        if success {
                            self.tx_evt
                                .send(AudioEngineEvent::RenderingFinished { session_id: self.session_id.clone(),
                                                                            render_id: render.render_id.clone(),
                                                                            path });
                        } else {
                            let _ = std::fs::remove_file(&path);
                        }
                    }

                    if !success {
                        self.tx_evt
                            .send(AudioEngineEvent::RenderingFailed { session_id: self.session_id.clone(),
                                                                      render_id:  render.render_id.clone(),
                                                                      reason:     format!("rendering interrupted"), });
                    }

                    self.clear_all_tracks_parent_sends();
                    self.mode = EngineMode::Stopped;
                }
            }
            EngineMode::Playing(playing) => {
                if stopped_after_play {
                    self.clear_all_tracks_parent_sends();
                    self.mode = EngineMode::Stopped;
                }
            }
            EngineMode::Stopped => {}
        }
        self.play_state = new_play_state;
    }

    fn set_track_master_send(&mut self, track_id: ReaperTrackId, send: bool) {
        let reaper = Reaper::get();
        let index = match self.get_track_index(&track_id) {
            None => {
                warn!("Setting master send on track_id {track_id} but it does not exist anymore");
                return;
            }
            Some(index) => index,
        };

        unsafe {
            if let Some(track) = reaper.get_track(CurrentProject, index as u32) {
                reaper.set_media_track_info_value(track, TrackAttributeKey::MainSend, match send {
                          true => 1.0,
                          false => 0.0,
                      });
            }
        }
    }

    fn clear_track_media_items(&self, id: &ReaperTrackId) -> Option<String> {
        let index = self.get_track_index(id)?;
        let reaper = Reaper::get();
        if let Some(track) = reaper.get_track(CurrentProject, index as u32) {
            // find all media files
            let mut location = None;
            while let Some(media_item) = unsafe { reaper.get_track_media_item(track, 0) } {
                if let Some(take) = unsafe { reaper.get_active_take(media_item) } {
                    if let Some(pcm_source) = unsafe { reaper.get_media_item_take_source(take) } {
                        unsafe {
                            let mut c_buf = [0i8; 1024];
                            reaper.low()
                                  .GetMediaSourceFileName(pcm_source.as_ptr(), c_buf.as_mut_ptr(), 1024);
                            let as_string = String::from_utf8_lossy(mem::transmute(&c_buf[..]));
                            location = Some(as_string.to_string());
                        }
                    }
                }
                unsafe {
                    reaper.delete_track_media_item(track, media_item);
                }
            }

            location
        } else {
            None
        }
    }

    fn clear_all_tracks_parent_sends(&self) {
        let reaper = Reaper::get();
        for i in 0.. {
            match reaper.get_track(CurrentProject, i as u32) {
                Some(track) => unsafe {
                    reaper.set_media_track_info_value(track, TrackAttributeKey::MainSend, 0.0);
                },
                None => break,
            }
        }
    }

    fn get_track_index(&self, id: &ReaperTrackId) -> Option<usize> {
        self.reaper_tracks.iter().position(|track_id| track_id == id)
    }

    fn get_track_metering(&self) -> HashMap<ReaperTrackId, MultiChannelTimestampedValue> {
        let reaper = Reaper::get();
        let mut metering = HashMap::new();

        for track in 0..reaper.count_tracks(CurrentProject) {
            let track = reaper.get_track(CurrentProject, track).unwrap();
            let num_channels = unsafe { reaper.get_media_track_info_value(track, TrackAttributeKey::Nchan) as u32 };

            let mcv = MultiChannelTimestampedValue::with_capacity(num_channels as usize);
            for channel in 0..num_channels {
                let volume = unsafe { reaper.track_get_peak_info(track, channel) }.get();
                mcv.push(Some(Timestamped::new(ModelValue::Number(volume))));
            }

            let name = unsafe {
                reaper.get_set_media_track_info_get_name(track, |name| ReaperTrackId::from_str(name.to_str()))
            };

            if let Some(Ok(track_id)) = name {
                metering.insert(track_id, mcv);
            }
        }

        metering
    }
}
