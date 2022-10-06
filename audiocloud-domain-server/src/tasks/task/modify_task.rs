use actix::Handler;

use audiocloud_api::domain::tasks::TaskUpdated;

use crate::tasks::task::TaskActor;
use crate::tasks::ModifyTask;
use crate::DomainResult;

impl Handler<ModifyTask> for TaskActor {
    type Result = DomainResult<TaskUpdated>;

    fn handle(&mut self, msg: ModifyTask, ctx: &mut Self::Context) -> Self::Result {
        todo!()
    }
}
