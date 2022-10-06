#![allow(unused_variables)]

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use actix::{Actor, Addr, AsyncContext, Context, Handler, Supervised, Supervisor};
use actix_broker::{BrokerIssue, BrokerSubscribe};
use tracing::*;

use audiocloud_api::cloud::domains::{DomainConfig, DomainEngineConfig, FixedInstanceRoutingMap};
use audiocloud_api::common::change::TaskState;
use audiocloud_api::domain::DomainError;
use audiocloud_api::newtypes::{AppTaskId, EngineId};
use audiocloud_api::{now, DomainId, FixedInstanceId, TaskPermissions, TaskReservation, TaskSecurity, TaskSpec};

use crate::db::Db;
use crate::tasks::messages::{BecomeOnline, NotifyTaskSpec, NotifyTaskState};
use crate::tasks::task::TaskActor;
use crate::tasks::{
    NotifyTaskActivated, NotifyTaskDeactivated, NotifyTaskDeleted, NotifyTaskReservation, NotifyTaskSecurity, TaskOpts,
};
use crate::{DomainResult, DomainSecurity};

const ALLOW_MODIFY_STRUCTURE: TaskPermissions = TaskPermissions { structure: true,
                                                                  ..TaskPermissions::empty() };

mod create_task;
mod delete_task;
mod get_task;
mod handle_engine_events;
mod handle_instance_events;
mod handle_media_events;
mod handle_task_events;
mod list_tasks;
mod modify_task;
mod play_task;
mod render_task;

pub struct TasksSupervisor {
    db:                        Db,
    task_opts:                 TaskOpts,
    domain_config:             DomainConfig,
    tasks:                     HashMap<AppTaskId, SupervisedTask>,
    engines:                   HashMap<EngineId, ReferencedEngine>,
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

struct ReferencedEngine {
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
         SupervisedTask {
           domain_id: { task.domain_id.clone() },
           reservations: { task.reservations.clone() },
           spec: { task.spec.clone() },
           security: { task.security.clone() },
           state: { Default::default() },
           actor: { None },
         })
      })
      .collect();

        let engines = cfg.engines
                         .iter()
                         .map(|(id, config)| (id.clone(), ReferencedEngine { config: config.clone() }))
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
        // TODO: we know we only have one engine, so we always pick the first
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
                                         self.task_opts.clone(),
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
        self.subscribe_task_events(ctx);
        self.subscribe_instance_events(ctx);
        self.subscribe_media_events(ctx);
        self.subscribe_engine_events(ctx);

        ctx.run_interval(Duration::from_millis(100), Self::update);
    }
}

impl Handler<BecomeOnline> for TasksSupervisor {
    type Result = ();

    fn handle(&mut self, msg: BecomeOnline, ctx: &mut Self::Context) -> Self::Result {
        self.online = true;
    }
}

fn check_security(task_id: &AppTaskId,
                  task: &TaskSecurity,
                  security: &DomainSecurity,
                  permissions: TaskPermissions)
                  -> DomainResult {
    match security {
        DomainSecurity::Cloud => Ok(()),
        DomainSecurity::SecureKey(secure_key) => match task.security.get(secure_key) {
            None => Err(DomainError::AuthenticationFailed),
            Some(security) => {
                if security.can(permissions) {
                    Ok(())
                } else {
                    Err(DomainError::TaskAuthtorizationFailed { task_id:  task_id.clone(),
                                                                required: permissions, })
                }
            }
        },
    }
}
