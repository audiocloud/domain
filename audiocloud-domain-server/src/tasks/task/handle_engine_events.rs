use std::collections::HashMap;

use actix::Handler;

use audiocloud_api::audio_engine::EngineEvent;
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
                if &self.id == &task_id {
                    self.engine.set_actual_playing(play_id);
                    self.merge_input_peak_meters(input_peak_meters);
                    self.merge_output_peak_meters(output_peak_meters);
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

impl TaskActor {
    fn merge_input_peak_meters(&mut self, input_peak_meters: HashMap<DestinationPadId, PadMetering>) {
        for (pad_id, metering) in input_peak_meters {
            self.packet
                .node_inputs
                .entry(pad_id)
                .or_default()
                .push(DiffStamped::new(self.packet.created_at, metering));
        }
    }

    pub fn merge_output_peak_meters(&mut self, output_peak_meters: HashMap<SourcePadId, PadMetering>) {
        for (pad_id, metering) in output_peak_meters {
            self.packet
                .node_outputs
                .entry(pad_id)
                .or_default()
                .push(DiffStamped::new(self.packet.created_at, metering));
        }
    }
}
