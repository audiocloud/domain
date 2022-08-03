use anyhow::anyhow;
use audiocloud_api::change::{PlaySession, RenderSession};

use std::collections::HashMap;
use std::mem;
use std::path::PathBuf;
use std::str::FromStr;
use tracing::*;

use audiocloud_api::model::{ModelValue, MultiChannelTimestampedValue};
use audiocloud_api::newtypes::{MixerId, TrackId};
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
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReaperTrackId {
    Input(SessionObjectId),
    Output(SessionObjectId),
}

#[derive(Debug)]
pub enum EngineMode {
    Stopped,
    Rendering(RenderSession),
    Playing(PlaySession),
}

impl ToString for ReaperTrackId {
    fn to_string(&self) -> String {
        match self {
            ReaperTrackId::Input(obj_id) => format!("inp:{obj_id}"),
            ReaperTrackId::Output(obj_id) => format!("out:{obj_id}"),
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
                AudioEngineCommand::SetSpec { session_id, spec } => self.set_sepc(spec),
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

        // send all the metering
        if let Err(err) = self.tx_mtr.try_send(metering) {
            warn!(%err, "Could not deliver metering");
        }

        // update current play state, update mode, emit events if needed
        self.update_play_state();
    }
}

impl AudiocloudControlSurface {
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
                    let path = self.get_last_media_item_path(&render.mixer_id);
                    if let Some(path) = path && success {
                        self.tx_evt.send(AudioEngineEvent::RenderingFinished { session_id: self.session_id.clone(), render_id: render.render_id.clone(), path });
                    } else {
                        if let Some(path) = path {
                            let _ = std::fs::remove_file(&path);
                        }
                        self.tx_evt.send(AudioEngineEvent::RenderingFailed {
                            session_id: self.session_id.clone(),
                            render_id: render.render_id.clone(),
                            reason: format!("rendering interrupted")
                        });
                    }
                    self.cleanup_play(&render.mixer_id);
                    self.mode = EngineMode::Stopped;
                }
            }
            EngineMode::Playing(playing) => {
                if stopped_after_play {
                    self.cleanup_play(&playing.mixer_id);
                    self.mode = EngineMode::Stopped;
                }
            }
            EngineMode::Stopped => {}
        }
        self.play_state = new_play_state;
    }

    fn cleanup_play(&mut self, mixer_id: &MixerId) {
        self.set_track_master_send(ReaperTrackId::Output(SessionObjectId::Mixer(cur_mixer.clone())), false);
    }

    fn set_track_master_send(&mut self, track_id: ReaperTrackId, send: bool) {
        let index = self.reaper_tracks.iter().position(|id| {
                                                 if let &track_id = id {
                                                     cur_mixer == mixer_id
                                                 } else {
                                                     false
                                                 }
                                             });

        let index = match index {
            None => {
                warn!("Setting master send on track_id {track_id} but it does not exist anymore");
                return;
            }
            Some(index) => index,
        };

        let reaper = Reaper::get();

        // "un-send to master"
        unsafe {
            if let Some(track) = reaper.get_track(CurrentProject, index as u32) {
                reaper.set_media_track_info_value(track, TrackAttributeKey::MainSend, match send {
                          true => 1.0,
                          false => 0.0,
                      });
            }
        }
    }

    fn get_last_media_item_path(&self, index: u32) -> Option<String> {
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
}
