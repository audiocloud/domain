use anyhow::anyhow;
use audiocloud_api::change::{ModifySessionSpec, PlayId, PlaySession, RenderSession};
use flume::{Receiver, Sender};
use maplit::hashmap;

use std::collections::HashMap;
use std::fmt::Display;
use std::mem;
use std::str::FromStr;
use tracing::*;

use audiocloud_api::model::{ModelValue, MultiChannelTimestampedValue, MultiChannelValue};
use audiocloud_api::newtypes::{AppSessionId, DynamicId, ParameterId, TrackId};
use audiocloud_api::session::{SessionObjectId, SessionTrack};
use audiocloud_api::time::Timestamped;
use reaper_medium::ProjectContext::CurrentProject;
use reaper_medium::{ChunkCacheHint, ControlSurface, PlayState, Reaper, TrackAttributeKey, TrackDefaultsBehavior};

use audiocloud_api::audio_engine::{AudioEngineCommand, AudioEngineEvent};
use audiocloud_api::cloud::apps::SessionSpec;

use crate::events::{ControlSurfaceCommandWithResultSender, ControlSurfaceEvent};
use crate::streaming::StreamingConfig;

pub mod track;

#[derive(Debug)]
pub struct AudiocloudControlSurface {
    rx_cmd:        flume::Receiver<ControlSurfaceCommandWithResultSender>,
    tx_evt:        flume::Sender<ControlSurfaceEvent>,
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
        while let Ok((msg, result)) = self.rx_cmd.try_recv() {
            result.send(match msg {
                            crate::events::ControlSurfaceCommand::Engine(msg) => self.exec_audio_engine_cmd(msg),
                            crate::events::ControlSurfaceCommand::StreamingSetupComplete(play_id) => {
                                self.on_streaming_setup_complete(play_id)
                            }
                            crate::events::ControlSurfaceCommand::StreamingSetupError(play_id, err) => {
                                self.on_streaming_setup_error(play_id, err)
                            }
                        });
        }

        // take measurements from peak meters and send to integration task
        if let Err(err) = self.tx_mtr.try_send(self.get_track_metering()) {
            warn!(%err, "Could not deliver metering");
        }

        // update current play state from REAPER, update engine mode, emit events if needed
        self.update_play_state();
    }
}

impl AudiocloudControlSurface {
    pub fn new(session_id: AppSessionId,
               spec: SessionSpec,
               rx_cmd: Receiver<ControlSurfaceCommandWithResultSender>,
               tx_evt: Sender<ControlSurfaceEvent>,
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

    fn exec_audio_engine_cmd(&mut self, cmd: AudioEngineCommand) -> anyhow::Result<()> {
        match cmd {
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

    fn on_streaming_setup_complete(&mut self, play_id: PlayId) -> anyhow::Result<()> {
        if let EngineMode::Playing(playing) = &self.mode {
            if playing.play_id == play_id {
                Reaper::get().on_play_button_ex(CurrentProject);
            }
        }

        Ok(())
    }

    fn on_streaming_setup_error(&mut self, play_id: PlayId, err: String) -> anyhow::Result<()> {
        if let EngineMode::Playing(playing) = &self.mode {
            if playing.play_id == play_id {
                let event = AudioEngineEvent::PlayingFailed { session_id: self.session_id.clone(),
                                                              play_id:    playing.play_id.clone(),
                                                              error:      err, };

                self.tx_evt.send(ControlSurfaceEvent::EngineEvent(event))?;

                self.stop();
                self.update_play_state();
            }
        }

        Ok(())
    }

    fn set_spec(&mut self, spec: SessionSpec) -> anyhow::Result<()> {
        self.stop();
        self.update_play_state();

        let prev_spec = self.spec;

        if let Err(err) = self.set_spec_no_retry(spec) {
            self.set_spec_no_retry(prev_spec)?;
        }

        Ok(())
    }

    fn set_spec_no_retry(&mut self, spec: SessionSpec) -> anyhow::Result<()> {
        self.clear_all_tracks();
        for (track_id, spec) in spec.tracks {
            self.add_track(track_id, spec)?;
        }

        Ok(())
    }

    fn modify_spec(&mut self, transaction: Vec<ModifySessionSpec>) -> anyhow::Result<()> {
        let existing_spec = self.spec.clone();

        for spec in transaction {
            if let Err(err) = self.modify_spec_one(spec) {
                self.set_spec(existing_spec)?;
                return Err(err);
            }
        }

        Ok(())
    }

    fn modify_spec_one(&mut self, modify: ModifySessionSpec) -> anyhow::Result<()> {
        match modify {
            ModifySessionSpec::AddTrack { track_id, channels } => {
                let spec = SessionTrack { channels,
                                          media: HashMap::new() };
                self.add_track(track_id, spec)?;
            }
            ModifySessionSpec::AddTrackMedia { track_id,
                                               media_id,
                                               channels,
                                               media_segment,
                                               timeline_segment,
                                               object_id,
                                               format, } => todo!(),
            ModifySessionSpec::SetTrackMediaValues { track_id,
                                                     media_id,
                                                     channels,
                                                     media_segment,
                                                     timeline_segment,
                                                     object_id, } => todo!(),
            ModifySessionSpec::DeleteTrackMedia { track_id, media_id } => todo!(),
            ModifySessionSpec::DeleteTrack { track_id } => todo!(),
            ModifySessionSpec::AddFixedInstance { fixed_id, process } => todo!(),
            ModifySessionSpec::AddDynamicInstance { dynamic_id, process } => todo!(),
            ModifySessionSpec::AddMixer { mixer_id, mixer } => todo!(),
            ModifySessionSpec::DeleteMixer { mixer_id } => todo!(),
            ModifySessionSpec::DeleteMixerInput { mixer_id, input_id } => todo!(),
            ModifySessionSpec::DeleteInputsReferencing { source_id } => todo!(),
            ModifySessionSpec::AddMixerInput { mixer_id,
                                               input_id,
                                               input, } => todo!(),
            ModifySessionSpec::SetInputValues { mixer_id,
                                                input_id,
                                                values, } => todo!(),
            ModifySessionSpec::SetFixedInstanceParameterValues { fixed_id, values } => todo!(),
            ModifySessionSpec::SetDynamicInstanceParameterValues { dynamic_id, values } => todo!(),
        }

        Ok(())
    }

    fn set_dynamic_parameters(&self,
                              dynamic_id: DynamicId,
                              parameters: HashMap<ParameterId, MultiChannelValue>)
                              -> anyhow::Result<()> {
        Ok(())
    }

    fn start_render(&mut self, render: RenderSession) -> anyhow::Result<()> {
        self.stop();
        self.update_play_state();

        let track_id = ReaperTrackId::Output(SessionObjectId::Mixer(render.mixer_id.clone()));
        self.set_track_master_send(&track_id, true)?;
        self.set_track_record_arm(&track_id, true)?;

        // arm for recording

        self.mode = EngineMode::Rendering(render);

        Ok(())
    }

    fn start_play(&mut self, play: PlaySession) -> anyhow::Result<()> {
        let mixer_id = &play.mixer_id;
        let mixer = self.spec
                        .mixers
                        .get(&play.mixer_id)
                        .ok_or_else(|| anyhow!("No such mixer {mixer_id}"))?;

        self.stop();
        self.update_play_state();

        self.tx_evt
            .send(ControlSurfaceEvent::SetupStreaming(Some(StreamingConfig { channels:    mixer.channels.max(2),
                                                                             sample_rate: play.sample_rate.into(),
                                                                             bit_depth:   play.bit_depth.into(),
                                                                             play_id:     play.play_id.clone(), })))?;

        let track_id = ReaperTrackId::Output(SessionObjectId::Mixer(play.mixer_id.clone()));
        self.set_track_master_send(&track_id, true)?;

        self.mode = EngineMode::Playing(play);

        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        Reaper::get().on_stop_button_ex(CurrentProject);
        self.tx_evt.send(ControlSurfaceEvent::SetupStreaming(None))?;

        Ok(())
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
                                .send(ControlSurfaceEvent::EngineEvent(AudioEngineEvent::RenderingFinished { session_id: self.session_id.clone(),
                                                                            render_id: render.render_id.clone(),
                                                                            path }));
                        } else {
                            let _ = std::fs::remove_file(&path);
                        }
                    }

                    if !success {
                        self.tx_evt
                            .send(ControlSurfaceEvent::EngineEvent(AudioEngineEvent::RenderingFailed { session_id: self.session_id.clone(),
                                                                      render_id:  render.render_id.clone(),
                                                                      reason:     format!("rendering interrupted"), }));
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

    fn set_track_master_send(&mut self, track_id: &ReaperTrackId, send: bool) -> anyhow::Result<()> {
        let reaper = Reaper::get();
        let index = self.get_track_index(track_id)
                        .ok_or_else(|| anyhow!("Setting master send on track {track_id} but it does not exist"))?;

        unsafe {
            if let Some(track) = reaper.get_track(CurrentProject, index as u32) {
                reaper.set_media_track_info_value(track, TrackAttributeKey::MainSend, match send {
                          true => 1.0,
                          false => 0.0,
                      });
            } else {
                return Err(anyhow!("Setting master send on track {track_id} but it does not exist"));
            }
        }

        Ok(())
    }

    fn set_track_record_arm(&mut self, track_id: &ReaperTrackId, rec_arm: bool) -> anyhow::Result<()> {
        let reaper = Reaper::get();
        let index = self.get_track_index(track_id)
                        .ok_or_else(|| anyhow!("Setting record arm send on track {track_id} but it does not exist"))?;

        unsafe {
            if let Some(track) = reaper.get_track(CurrentProject, index as u32) {
                reaper.set_media_track_info_value(track, TrackAttributeKey::RecArm, match rec_arm {
                          true => 1.0,
                          false => 0.0,
                      });
            } else {
                return Err(anyhow!("Setting master send on track {track_id} but it does not exist"));
            }
        }

        Ok(())
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

    fn clear_all_tracks(&mut self) {
        self.tracks.clear();
        self.reaper_tracks.clear();
        let reaper = Reaper::get();

        while let Some(track) = reaper.get_track(CurrentProject, 0) {
            unsafe {
                reaper.delete_track(track);
            }
        }
    }

    fn add_track(&mut self, track_id: TrackId, spec: SessionTrack) -> anyhow::Result<()> {
        let reaper = Reaper::get();
        let track_index = reaper.count_tracks(CurrentProject);
        reaper.insert_track_at_index(track_index, TrackDefaultsBehavior::OmitDefaultEnvAndFx);

        let reaper_track = reaper.get_track(CurrentProject, track_index)
                                 .ok_or_else(|| anyhow!("Failed to get track we just inserted"))?;

        unsafe {
            reaper.set_media_track_info_value(reaper_track,
                                              TrackAttributeKey::Nchan,
                                              spec.channels.num_channels() as f64)?
        };

        // this is very fragile, because we are assuming that the out of order update of the reaper_tracks is enough to convince set_track_chunk
        // to work. If we ever change what set_track_chunk reads, this will break.

        let reaper_track_id = ReaperTrackId::Output(SessionObjectId::Track(track_id.clone()));

        self.reaper_tracks.push(reaper_track_id.clone());

        let track = track::ReaperTrack::new(track_id.clone(), spec.clone());

        self.set_track_chunk(reaper_track_id, track.get_chunk()?)?;

        self.tracks.insert(track_id.clone(), track);

        Ok(())
    }

    fn set_track_chunk(&mut self, reaper_track_id: ReaperTrackId, chunk: String) -> anyhow::Result<()> {
        let reaper = Reaper::get();

        let index = self.get_track_index(&reaper_track_id)
                        .ok_or_else(|| anyhow!("Track {reaper_track_id} not found and cannot be updated"))?;

        let track = reaper.get_track(CurrentProject, index as u32)
                          .ok_or_else(|| anyhow!("Failed to get track {reaper_track_id}"))?;

        unsafe {
            reaper.set_track_state_chunk(track, chunk, ChunkCacheHint::NormalMode)?;
        }

        Ok(())
    }
}
