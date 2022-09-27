use std::collections::HashMap;

use actix::fut::LocalBoxActorFuture;
use actix::{
    fut, Actor, ActorFutureExt, Addr, Context, ContextFutureSpawner, Handler, Supervised, Supervisor, SystemService,
    WrapFuture,
};
use actix_broker::BrokerSubscribe;
use anyhow::anyhow;
use tracing::warn;

use audiocloud_api::common::change::TaskState;
use audiocloud_api::common::task::Task;
use audiocloud_api::domain::tasks::TaskUpdated;
use audiocloud_api::domain::DomainError;
use audiocloud_api::newtypes::{AppTaskId, EngineId};

use crate::db::get_boot_cfg;
use crate::task::messages::{
    BecomeOnline, ExecuteTaskCommand, NotifyAudioEngineEvent, NotifyMediaTaskState, NotifyTaskSpec, NotifyTaskState,
    SetTaskDesiredState,
};
use crate::task::TaskActor;
use crate::DomainResult;

pub struct SessionsSupervisor {
    active:   HashMap<AppTaskId, Addr<TaskActor>>,
    sessions: HashMap<AppTaskId, Task>,
    state:    HashMap<AppTaskId, TaskState>,
    engines:  HashMap<EngineId, AudioEngineClient>,
    online:   bool,
}

impl SessionsSupervisor {
    fn allocate_engine(&self) -> Option<EngineId> {
        self.engines
            .iter()
            .min_by(|(_, engine_a), (_, engine_b)| engine_a.num_sessions().cmp(&engine_b.num_sessions()))
            .map(|(id, _)| id.clone())
    }
}

impl Actor for SessionsSupervisor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.restarting(ctx);
    }
}

impl Supervised for SessionsSupervisor {
    fn restarting(&mut self, ctx: &mut Self::Context) {
        self.subscribe_system_async::<NotifyTaskSpec>(ctx);
        self.subscribe_system_async::<NotifyTaskState>(ctx);
    }
}

impl Default for SessionsSupervisor {
    fn default() -> Self {
        let sessions = get_boot_cfg().tasks.clone();

        Self { sessions,
               online: false,
               active: Default::default(),
               state: Default::default(),
               engines: Default::default() }
    }
}

impl SystemService for SessionsSupervisor {}

impl Handler<NotifyTaskSpec> for SessionsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyTaskSpec, ctx: &mut Self::Context) -> Self::Result {
        if let Some(session) = self.sessions.get_mut(&msg.task_id) {
            session.spec = msg.spec;
        }
    }
}

impl Handler<SetTaskDesiredState> for SessionsSupervisor {
    type Result = LocalBoxActorFuture<Self, DomainResult<TaskUpdated>>;

    fn handle(&mut self, msg: SetTaskDesiredState, ctx: &mut Self::Context) -> Self::Result {
        if let Some(session) = self.active.get_mut(&msg.task_id) {
            session.send(msg).into_actor(self).boxed_local()
        } else {
            fut::err(DomainError::TaskNotFound { task_id: msg.task_id.clone(), }).into_actor(self)
                                                                                 .boxed_local()
        }
    }
}

impl Handler<BecomeOnline> for SessionsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: BecomeOnline, ctx: &mut Self::Context) -> Self::Result {
        if !self.online {
            self.online = true;
            for (id, session) in self.sessions.iter() {
                if session.reservations.contains_now() {
                    if let Some(engine_id) = self.allocate_engine() {
                        let actor = TaskActor::new(id, session, engine_id.clone());
                        self.active.insert(id.clone(), Supervisor::start(move |_| actor));
                    } else {
                        warn!(%id, "No available audio engines to start session");
                    }
                }
            }
        }
    }
}

impl Handler<NotifyTaskState> for SessionsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyTaskState, ctx: &mut Self::Context) -> Self::Result {
        self.state.insert(msg.session_id, msg.state);
    }
}

impl Handler<NotifyAudioEngineEvent> for SessionsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyAudioEngineEvent, ctx: &mut Self::Context) -> Self::Result {
        let session_id = msg.event.task_id();
        match self.active.get(session_id) {
            Some(session) => {
                session.do_send(msg);
            }
            None => {
                warn!(%session_id, "Dropping audio engine event for unknown / inactive session");
            }
        }
    }
}

impl Handler<NotifyMediaTaskState> for SessionsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyMediaTaskState, ctx: &mut Self::Context) -> Self::Result {
        let session_id = &msg.task_id;
        match self.active.get(session_id) {
            Some(session) => {
                session.do_send(msg);
            }
            None => {
                warn!(%session_id, "Dropping media service event for unknown / inactive session");
            }
        }
    }
}

impl Handler<ExecuteTaskCommand> for SessionsSupervisor {
    type Result = LocalBoxActorFuture<Self, anyhow::Result<()>>;

    fn handle(&mut self, msg: ExecuteTaskCommand, ctx: &mut Self::Context) -> Self::Result {
        if let Some(session) = self.active.get(&msg.session_id) {
            session.send(msg)
                   .into_actor(self)
                   .map(|res, _, _| anyhow::Result::<()>::Ok(res??))
                   .boxed_local()
        } else {
            fut::err(anyhow!("Session not found")).into_actor(self).boxed_local()
        }
    }
}
