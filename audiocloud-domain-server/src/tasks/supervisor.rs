use std::collections::HashMap;

use actix::fut::LocalBoxActorFuture;
use actix::{fut, Actor, ActorFutureExt, Addr, Context, Handler, Supervised, Supervisor, WrapFuture};
use actix_broker::BrokerSubscribe;
use tracing::warn;

use audiocloud_api::cloud::domains::{DomainConfig, DomainEngineConfig, FixedInstanceRoutingMap};
use audiocloud_api::common::change::TaskState;
use audiocloud_api::common::task::Task;
use audiocloud_api::domain::tasks::TaskUpdated;
use audiocloud_api::domain::DomainError;
use audiocloud_api::newtypes::{AppTaskId, EngineId};
use audiocloud_api::FixedInstanceId;

use crate::db::Db;
use crate::fixed_instances::NotifyFixedInstanceReports;
use crate::tasks::messages::{
    BecomeOnline, NotifyEngineEvent, NotifyMediaTaskState, NotifyTaskSpec, NotifyTaskState, SetTaskDesiredPlayState,
};
use crate::tasks::task::TaskActor;
use crate::DomainResult;

pub struct TasksSupervisor {
    db:                        Db,
    active:                    HashMap<AppTaskId, Addr<TaskActor>>,
    tasks:                     HashMap<AppTaskId, Task>,
    state:                     HashMap<AppTaskId, TaskState>,
    engines:                   HashMap<EngineId, Engine>,
    fixed_instance_membership: HashMap<FixedInstanceId, AppTaskId>,
    fixed_instance_routing:    FixedInstanceRoutingMap,
    online:                    bool,
}

struct Engine {
    config: DomainEngineConfig,
}

impl TasksSupervisor {
    pub fn new(db: Db, cfg: &DomainConfig, routing: FixedInstanceRoutingMap) -> anyhow::Result<Self> {
        let tasks = cfg.tasks.iter().map(|(id, task)| (id.clone(), task.clone())).collect();
        let engines = cfg.engines
                         .iter()
                         .map(|(id, config)| (id.clone(), Engine { config: config.clone() }))
                         .collect();

        Ok(Self { db:                        { db },
                  active:                    { HashMap::new() },
                  state:                     { HashMap::new() },
                  fixed_instance_membership: { HashMap::new() },
                  fixed_instance_routing:    { routing },
                  tasks:                     { tasks },
                  engines:                   { engines },
                  online:                    { false }, })
    }

    fn allocate_engine(&self) -> Option<EngineId> {
        self.engines.keys().next().cloned()
    }
}

impl Actor for TasksSupervisor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.restarting(ctx);
    }
}

impl Supervised for TasksSupervisor {
    fn restarting(&mut self, ctx: &mut Self::Context) {
        self.subscribe_system_async::<NotifyTaskSpec>(ctx);
        self.subscribe_system_async::<NotifyTaskState>(ctx);
        self.subscribe_system_async::<NotifyFixedInstanceReports>(ctx);
    }
}

impl Handler<NotifyTaskSpec> for TasksSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyTaskSpec, ctx: &mut Self::Context) -> Self::Result {
        // clear all previous associations with the same task ID
        self.fixed_instance_membership
            .retain(|_, task_id| task_id != &msg.task_id);

        // associate task ID with all the current fixed instance IDs
        for fixed_instance_id in msg.spec.get_fixed_instance_ids() {
            self.fixed_instance_membership
                .insert(fixed_instance_id.clone(), msg.task_id.clone());
        }

        if let Some(task) = self.tasks.get_mut(&msg.task_id) {
            task.spec = msg.spec;
        }
    }
}

impl Handler<NotifyFixedInstanceReports> for TasksSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyFixedInstanceReports, ctx: &mut Self::Context) -> Self::Result {
        if let Some(task_id) = self.fixed_instance_membership.get(&msg.instance_id) {
            if let Some(task_address) = self.active.get(task_id) {
                task_address.do_send(msg);
            }
        }
    }
}

impl Handler<SetTaskDesiredPlayState> for TasksSupervisor {
    type Result = LocalBoxActorFuture<Self, DomainResult<TaskUpdated>>;

    fn handle(&mut self, msg: SetTaskDesiredPlayState, ctx: &mut Self::Context) -> Self::Result {
        use DomainError::*;

        if let Some(task) = self.active.get_mut(&msg.task_id) {
            let task_id = msg.task_id.clone();
            task.send(msg)
                .into_actor(self)
                .map(move |res, actor, ctx| match res {
                    Ok(result) => result,
                    Err(err) => Err(BadGateway { error: format!("Task actor {task_id} failed: {err}"), }),
                })
                .boxed_local()
        } else {
            fut::err(TaskNotFound { task_id: msg.task_id.clone(), }).into_actor(self)
                                                                    .boxed_local()
        }
    }
}

impl Handler<BecomeOnline> for TasksSupervisor {
    type Result = ();

    fn handle(&mut self, msg: BecomeOnline, ctx: &mut Self::Context) -> Self::Result {
        if !self.online {
            self.online = true;
            for (id, session) in self.tasks.iter() {
                if session.reservations.contains_now() {
                    if let Some(engine_id) = self.allocate_engine() {
                        match TaskActor::new(id.clone(),
                                             engine_id.clone(),
                                             session.clone(),
                                             self.fixed_instance_routing.clone())
                        {
                            Ok(actor) => {
                                self.active.insert(id.clone(), Supervisor::start(move |_| actor));
                            }
                            Err(error) => {
                                warn!(%error, "Failed to start task");
                            }
                        }
                    } else {
                        warn!(%id, "No available audio engines to start session");
                    }
                }
            }
        }
    }
}

impl Handler<NotifyTaskState> for TasksSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyTaskState, ctx: &mut Self::Context) -> Self::Result {
        self.state.insert(msg.session_id, msg.state);
    }
}

impl Handler<NotifyEngineEvent> for TasksSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyEngineEvent, ctx: &mut Self::Context) -> Self::Result {
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

impl Handler<NotifyMediaTaskState> for TasksSupervisor {
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
