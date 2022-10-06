use actix::{Handler, MessageResult};

use audiocloud_api::domain::tasks::TaskUpdated;
use audiocloud_api::{DesiredInstancePlayState, DesiredTaskPlayState};

use crate::tasks::task::TaskActor;
use crate::tasks::SetTaskDesiredPlayState;

impl Handler<SetTaskDesiredPlayState> for TaskActor {
    type Result = MessageResult<SetTaskDesiredPlayState>;

    fn handle(&mut self, msg: SetTaskDesiredPlayState, ctx: &mut Self::Context) -> Self::Result {
        let desired_instance_state = match &msg.desired {
            DesiredTaskPlayState::Stopped => DesiredInstancePlayState::Stopped,
            DesiredTaskPlayState::Play(play) => DesiredInstancePlayState::Playing { play_id: play.play_id },
            DesiredTaskPlayState::Render(render) => DesiredInstancePlayState::Rendering { render_id: render.render_id,
                                                                                          length:    render.segment
                                                                                                           .length, },
        };

        self.fixed_instances.set_desired_state(desired_instance_state);
        let version = self.engine.set_desired_state(msg.desired);

        MessageResult(Ok(TaskUpdated::Updated { app_id:   { self.id.app_id.clone() },
                                                task_id:  { self.id.task_id.clone() },
                                                revision: { version }, }))
    }
}
