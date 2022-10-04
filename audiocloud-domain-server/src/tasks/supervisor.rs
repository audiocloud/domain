use std::collections::HashMap;

use actix::fut::LocalBoxActorFuture;
use actix::{fut, Actor, ActorFutureExt, Addr, Context, Handler, Supervised, Supervisor, WrapFuture};
use actix_broker::BrokerSubscribe;
use tracing::warn;

use audiocloud_api::cloud::domains::{DomainConfig, DomainEngineConfig, FixedInstanceRoutingMap};
use audiocloud_api::common::change::TaskState;
use audiocloud_api::domain::tasks::{TaskCreated, TaskSummary, TaskSummaryList, TaskUpdated, TaskWithStatusAndSpec};
use audiocloud_api::domain::DomainError;
use audiocloud_api::newtypes::{AppTaskId, EngineId};
use audiocloud_api::{DomainId, FixedInstanceId, TaskReservation, TaskSecurity, TaskSpec};

use crate::db::Db;
use crate::fixed_instances::NotifyFixedInstanceReports;
use crate::tasks::messages::{
    BecomeOnline, NotifyEngineEvent, NotifyMediaTaskState, NotifyTaskSpec, NotifyTaskState, SetTaskDesiredPlayState,
};
use crate::tasks::task::TaskActor;
use crate::tasks::{CreateTask, GetTaskWithStatusAndSpec, ListTasks};
use crate::DomainResult;

pub struct TasksSupervisor {
    db:                        Db,
    tasks:                     HashMap<AppTaskId, SupervisedTask>,
    engines:                   HashMap<EngineId, Engine>,
    fixed_instance_membership: HashMap<FixedInstanceId, AppTaskId>,
    fixed_instance_routing:    FixedInstanceRoutingMap,
    online:                    bool,
}

struct SupervisedTask {
    pub domain_id:    DomainId,
    pub reservations: TaskReservation,
    pub spec:         TaskSpec,
    pub security:     TaskSecurity,
    pub state:        TaskState,
    pub actor:        Option<Addr<TaskActor>>,
}

struct Engine {
    config: DomainEngineConfig,
}

impl TasksSupervisor {
    pub fn new(db: Db, cfg: &DomainConfig, routing: FixedInstanceRoutingMap) -> anyhow::Result<Self> {
        let tasks = cfg.tasks
                       .iter()
                       .map(|(id, task)| {
                           (id.clone(),
                            SupervisedTask { domain_id:    { task.domain_id.clone() },
                                             reservations: { task.reservations.clone() },
                                             spec:         { task.spec.clone() },
                                             security:     { task.security.clone() },
                                             state:        { Default::default() },
                                             actor:        { None }, })
                       })
                       .collect();

        let engines = cfg.engines
                         .iter()
                         .map(|(id, config)| (id.clone(), Engine { config: config.clone() }))
                         .collect();

        Ok(Self { db:                        { db },
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
            if let Some(actor_addr) = self.tasks.get(task_id).and_then(|task| task.actor.as_ref()) {
                actor_addr.do_send(msg);
            }
        }
    }
}

impl Handler<SetTaskDesiredPlayState> for TasksSupervisor {
    type Result = LocalBoxActorFuture<Self, DomainResult<TaskUpdated>>;

    fn handle(&mut self, msg: SetTaskDesiredPlayState, ctx: &mut Self::Context) -> Self::Result {
        use DomainError::*;

        if let Some(task) = self.tasks.get(&msg.task_id).and_then(|task| task.actor.as_ref()) {
            let task_id = msg.task_id.clone();
            task.send(msg)
                .into_actor(self)
                .map(move |res, actor, ctx| match res {
                    Ok(result) => result,
                    Err(err) => {
                        Err(BadGateway { error: format!("Task actor {task_id} failed to set desired state: {err}"), })
                    }
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

            // generate an actor map to later assign
            let mut actors = HashMap::new();

            for (id, task) in self.tasks.iter() {
                if task.reservations.contains_now() {
                    if let Some(engine_id) = self.allocate_engine() {
                        match TaskActor::new(id.clone(),
                                             task.domain_id.clone(),
                                             engine_id.clone(),
                                             task.reservations.clone(),
                                             task.spec.clone(),
                                             task.security.clone(),
                                             self.fixed_instance_routing.clone())
                        {
                            Ok(actor) => {
                                actors.insert(id.clone(), Supervisor::start(move |_| actor));
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

            for (id, task) in self.tasks.iter_mut() {
                if let Some(actor) = actors.remove(&id) {
                    task.actor.replace(actor);
                }
            }
        }
    }
}

impl Handler<NotifyTaskState> for TasksSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyTaskState, ctx: &mut Self::Context) -> Self::Result {
        if let Some(task) = self.tasks.get_mut(&msg.task_id) {
            task.state = msg.state;
        }
    }
}

impl Handler<NotifyEngineEvent> for TasksSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyEngineEvent, ctx: &mut Self::Context) -> Self::Result {
        let session_id = msg.event.task_id();
        match self.tasks.get(session_id).and_then(|task| task.actor.as_ref()) {
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
        match self.tasks.get(session_id).and_then(|task| task.actor.as_ref()) {
            Some(session) => {
                session.do_send(msg);
            }
            None => {
                warn!(%session_id, "Dropping media service event for unknown / inactive session");
            }
        }
    }
}

impl Handler<ListTasks> for TasksSupervisor {
    type Result = TaskSummaryList;

    fn handle(&mut self, msg: ListTasks, ctx: &mut Self::Context) -> Self::Result {
        let mut rv = vec![];
        for (id, task) in &self.tasks {
            // TODO: missing `waiting_for_instances` and `waiting_for_media`
            // TODO: would be nice if TaskSummary included timestamps
            rv.push(TaskSummary { app_id:                id.app_id.clone(),
                                  task_id:               id.task_id.clone(),
                                  play_state:            task.state.play_state.value().clone(),
                                  waiting_for_instances: Default::default(),
                                  waiting_for_media:     Default::default(), });
        }

        rv
    }
}

impl Handler<CreateTask> for TasksSupervisor {
    type Result = DomainResult<TaskCreated>;

    fn handle(&mut self, msg: CreateTask, ctx: &mut Self::Context) -> Self::Result {
        todo!()
    }
}

impl Handler<GetTaskWithStatusAndSpec> for TasksSupervisor {
    type Result = DomainResult<TaskWithStatusAndSpec>;

    fn handle(&mut self, msg: GetTaskWithStatusAndSpec, ctx: &mut Self::Context) -> Self::Result {
        if let Some(task) = self.tasks.get(&msg.task_id) {
            Ok(TaskWithStatusAndSpec { app_id:     msg.task_id.app_id.clone(),
                                       task_id:    msg.task_id.task_id.clone(),
                                       play_state: task.state.play_state.value().clone(),
                                       instances:  Default::default(),
                                       media:      Default::default(),
                                       spec:       Default::default(), })
        } else {
            Err(DomainError::TaskNotFound { task_id: msg.task_id })
        }
    }
}
