use std::collections::HashMap;

use actix::Handler;

use audiocloud_api::audio_engine::{CompressedAudio, EngineEvent};
use audiocloud_api::domain::streaming::DiffStamped;
use audiocloud_api::{DesiredTaskPlayState, DestinationPadId, PadMetering, SourcePadId};

use crate::tasks::task::TaskActor;
use crate::tasks::NotifyEngineEvent;

impl Handler<NotifyEngineEvent> for TaskActor {
    type Result = ();

    fn handle(&mut self, msg: NotifyEngineEvent, ctx: &mut Self::Context) -> Self::Result {
        use EngineEvent::*;

        if &self.engine_id != &msg.engine_id {
            return;
        }

        match msg.event {
            Stopped { task_id } => {
                if &self.id == &task_id {
                    self.engine.set_actual_stopped();
                }
            }
            Playing { task_id,
                      play_id,
                      audio,
                      output_peak_meters,
                      input_peak_meters,
                      dynamic_reports, } => {
                if &self.id == &task_id && self.engine.should_be_playing(&play_id) {
                    self.engine.set_actual_playing(play_id);
                    self.merge_input_peak_meters(input_peak_meters);
                    self.merge_output_peak_meters(output_peak_meters);
                    self.push_compressed_audio(audio);
                    self.maybe_send_packet();
                }
            }
            PlayingFailed { task_id,
                            play_id,
                            error, } => {
                if &self.id == &task_id {
                    self.engine.set_desired_state(DesiredTaskPlayState::Stopped);
                    self.engine.set_actual_stopped();
                }
            }
            Rendering { task_id,
                        render_id,
                        completion, } => {
                if &self.id == &task_id {
                    self.engine.set_actual_rendering(render_id);
                }
            }
            RenderingFinished { task_id,
                                render_id,
                                path, } => {
                if &self.id == &task_id {
                    self.engine.set_desired_state(DesiredTaskPlayState::Stopped);
                    self.engine.set_actual_stopped();
                }
            }
            RenderingFailed { task_id,
                              render_id,
                              error, } => {
                if &self.id == &task_id {
                    self.engine.set_desired_state(DesiredTaskPlayState::Stopped);
                    self.engine.set_actual_stopped();
                }
            }
            Error { task_id, error } => {
                if &self.id == &task_id {
                    // do not modify desired states..
                }
            }
        }
    }
}