#![allow(unused_variables)]

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use actix::fut::LocalBoxActorFuture;
use actix::{
    fut, Actor, ActorFutureExt, ActorTryFutureExt, Addr, AsyncContext, Context, Handler, Supervised, Supervisor,
    WrapFuture,
};
use actix_broker::{BrokerIssue, BrokerSubscribe};
use clap::Args;
use futures::FutureExt;
use tracing::*;

use audiocloud_api::cloud::domains::{DomainConfig, DomainEngineConfig, FixedInstanceRoutingMap};
use audiocloud_api::common::change::TaskState;
use audiocloud_api::domain::tasks::{TaskCreated, TaskSummary, TaskSummaryList, TaskUpdated, TaskWithStatusAndSpec};
use audiocloud_api::domain::DomainError;
use audiocloud_api::newtypes::{AppTaskId, EngineId};
use audiocloud_api::{now, DomainId, FixedInstanceId, TaskReservation, TaskSecurity, TaskSpec};

use crate::db::Db;
use crate::fixed_instances::NotifyFixedInstanceReports;
use crate::tasks::messages::{
    BecomeOnline, NotifyEngineEvent, NotifyMediaTaskState, NotifyTaskSpec, NotifyTaskState, SetTaskDesiredPlayState,
};
use crate::tasks::task::TaskActor;
use crate::tasks::{
    CreateTask, GetTaskWithStatusAndSpec, ListTasks, ModifyTask, NotifyTaskActivated, NotifyTaskDeactivated,
    NotifyTaskDeleted, NotifyTaskReservation, NotifyTaskSecurity,
};
use crate::DomainResult;

#[derive(Args, Clone, Debug, Copy)]
pub struct TaskOpts {
    /// Number of seconds to keep task information in the supervisor before forgetting it
    #[clap(long, env, default_value = "3600")]
    pub task_grace_seconds: usize,
}

pub struct TasksSupervisor {
    db:                        Db,
    task_opts:                 TaskOpts,
    domain_config:             DomainConfig,
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
    pub fn new(db: Db, opts: &TaskOpts, cfg: &DomainConfig, routing: FixedInstanceRoutingMap) -> anyhow::Result<Self> {
        let tasks = cfg.tasks
                       .iter()
                       .filter(|(id, task)| {
                           if &task.domain_id != &cfg.domain_id {
                               warn!(%id, domain_id = %cfg.domain_id, other_domain_id = %task.domain_id, "Configuration time task is for another domain, skipping");
                               true
                           } else {
                               false
                           }
                       })
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
                  task_opts:                 { opts.clone() },
                  domain_config:             { cfg.clone() },
                  fixed_instance_membership: { HashMap::new() },
                  fixed_instance_routing:    { routing },
                  tasks:                     { tasks },
                  engines:                   { engines },
                  online:                    { false }, })
    }

    fn allocate_engine(&self, id: &AppTaskId, spec: &TaskSpec) -> Option<EngineId> {
        // TODO: allocaet engine
        let engine_id = self.engines.keys().next().cloned();
        info!(?engine_id, %id, "Allocated engine for task");
        engine_id
    }

    fn update(&mut self, ctx: &mut Context<Self>) {
        if self.online {
            self.drop_inactive_task_actors();
            self.drop_old_tasks();
            self.create_pending_task_actors();
        }
    }

    fn drop_inactive_task_actors(&mut self) {
        let mut deactivated = HashSet::new();
        for (id, task) in &mut self.tasks {
            if task.actor.as_ref().map(|actor| !actor.connected()).unwrap_or(false) {
                debug!(%id, "Dropping task actor due to inactivity");
                deactivated.insert(id.clone());
                task.actor = None;
            }
        }

        for task_id in deactivated {
            self.issue_system_async(NotifyTaskDeactivated { task_id });
        }
    }

    fn drop_old_tasks(&mut self) {
        let mut deleted = HashSet::new();

        let cutoff = now() + chrono::Duration::seconds(self.task_opts.task_grace_seconds as i64);

        self.tasks.retain(|id, task| {
                      if task.reservations.to < cutoff {
                          deleted.insert(id.clone());
                          debug!(%id, "Cleaning up task from supervisor completely");
                          false
                      } else {
                          true
                      }
                  });

        for task_id in deleted {
            self.issue_system_async(NotifyTaskDeleted { task_id });
        }
    }

    fn create_pending_task_actors(&mut self) {
        // generate an actor map to later assign
        let mut actors = HashMap::new();

        for (id, task) in self.tasks.iter() {
            if task.reservations.contains_now() && task.actor.is_none() {
                if let Some(engine_id) = self.allocate_engine(&id, &task.spec) {
                    match TaskActor::new(id.clone(),
                                         task.domain_id.clone(),
                                         engine_id.clone(),
                                         task.reservations.clone(),
                                         task.spec.clone(),
                                         task.security.clone(),
                                         self.fixed_instance_routing.clone())
                    {
                        Ok(actor) => {
                            self.issue_system_async(NotifyTaskActivated { task_id: id.clone() });
                            actors.insert(id.clone(), Supervisor::start(move |_| actor));
                        }
                        Err(error) => {
                            warn!(%error, "Failed to start task");
                        }
                    }
                } else {
                    warn!(%id, "No available audio engines to start task");
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
        self.subscribe_system_async::<NotifyTaskReservation>(ctx);
        self.subscribe_system_async::<NotifyTaskSecurity>(ctx);
        self.subscribe_system_async::<NotifyFixedInstanceReports>(ctx);

        ctx.run_interval(Duration::from_millis(100), Self::update);
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
        self.online = true;
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

impl Handler<NotifyTaskReservation> for TasksSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyTaskReservation, ctx: &mut Self::Context) -> Self::Result {
        if let Some(task) = self.tasks.get_mut(&msg.task_id) {
            task.reservations = msg.reservation;
        }
    }
}

impl Handler<NotifyTaskSecurity> for TasksSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyTaskSecurity, ctx: &mut Self::Context) -> Self::Result {
        if let Some(task) = self.tasks.get_mut(&msg.task_id) {
            task.security = msg.security;
        }
    }
}

impl Handler<NotifyEngineEvent> for TasksSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyEngineEvent, ctx: &mut Self::Context) -> Self::Result {
        let task_id = msg.event.task_id();
        match self.tasks.get(task_id).and_then(|task| task.actor.as_ref()) {
            Some(session) => {
                session.do_send(msg);
            }
            None => {
                warn!(%task_id, "Dropping audio engine event for unknown / inactive task");
            }
        }
    }
}

impl Handler<NotifyMediaTaskState> for TasksSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyMediaTaskState, ctx: &mut Self::Context) -> Self::Result {
        let task_id = &msg.task_id;
        match self.tasks.get(task_id).and_then(|task| task.actor.as_ref()) {
            Some(task) => {
                task.do_send(msg);
            }
            None => {
                warn!(%task_id, "Dropping media service event for unknown / inactive task");
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
        if self.tasks.contains_key(&msg.task_id) {
            return Err(DomainError::TaskExists { task_id: msg.task_id });
        }

        self.tasks.insert(msg.task_id.clone(),
                          SupervisedTask { domain_id:    { self.domain_config.domain_id.clone() },
                                           reservations: { msg.reservations.into() },
                                           spec:         { msg.spec.into() },
                                           security:     { msg.security.into() },
                                           state:        { Default::default() },
                                           actor:        { None }, });

        self.update(ctx);

        Ok(TaskCreated::Created { task_id: msg.task_id.task_id.clone(),
                                  app_id:  msg.task_id.app_id.clone(), })
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

impl Handler<ModifyTask> for TasksSupervisor {
    type Result = LocalBoxActorFuture<Self, DomainResult<TaskUpdated>>;

    fn handle(&mut self, msg: ModifyTask, ctx: &mut Self::Context) -> Self::Result {
        match self.tasks.get(&msg.task_id).and_then(|task| task.actor.as_ref()) {
            Some(session) => session.send(msg)
                                    .into_actor(self)
                                    .map(|result, _, _| match result {
                                        Ok(result) => result,
                                        Err(err) => Err(DomainError::BadGateway { error: err.to_string() }),
                                    })
                                    .boxed_local(),
            None => {
                warn!(task_id = %msg.task_id, "Dropping audio engine event for unknown / inactive session");
                fut::err(DomainError::TaskNotFound { task_id: msg.task_id.clone(), }).into_actor(self)
                                                                                     .boxed_local()
            }
        }
    }
}
