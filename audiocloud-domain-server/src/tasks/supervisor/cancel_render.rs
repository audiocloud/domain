use actix::fut::LocalBoxActorFuture;
use actix::{fut, ActorFutureExt, Handler, WrapFuture};

use audiocloud_api::audio_engine::TaskRenderCancelled;
use audiocloud_api::domain::tasks::TaskUpdated;
use audiocloud_api::domain::DomainError;

use crate::tasks::CancelRenderTask;
use crate::DomainResult;

use super::TasksSupervisor;

impl Handler<CancelRenderTask> for TasksSupervisor {
    type Result = LocalBoxActorFuture<Self, DomainResult<TaskRenderCancelled>>;

    fn handle(&mut self, msg: CancelRenderTask, ctx: &mut Self::Context) -> Self::Result {
        use DomainError::*;

        if let Some(task) = self.tasks.get(&msg.task_id).and_then(|task| task.actor.as_ref()) {
            let task_id = msg.task_id.clone();
            task.send(msg)
                .into_actor(self)
                .map(move |res, actor, ctx| match res {
                    Ok(result) => result,
                    Err(err) => {
                        Err(BadGateway { error: format!("Task actor {task_id} failed to cancel render: {err}"), })
                    }
                })
                .boxed_local()
        } else {
            fut::err(TaskNotFound { task_id: msg.task_id.clone(), }).into_actor(self)
                                                                    .boxed_local()
        }
    }
}
