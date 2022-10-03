use actix::{Actor, Addr, AsyncContext, Handler, Supervised, Supervisor, SystemService};
use actix_broker::{BrokerIssue, BrokerSubscribe};
use anyhow::anyhow;
use once_cell::sync::OnceCell;

use audiocloud_api::cloud::domains::{DomainConfig, FixedInstanceRoutingMap};
pub use messages::*;
use supervisor::TasksSupervisor;

use crate::db::Db;

pub mod messages;
pub mod supervisor;
mod task;
mod task_engine;
mod task_fixed_instance;
mod task_media_objects;

static TASKS_SUPERVISOR: OnceCell<Addr<TasksSupervisor>> = OnceCell::new();

pub fn get_tasks_supervisor() -> &'static Addr<TasksSupervisor> {
    TASKS_SUPERVISOR.get().expect("Tasks supervisor not initialized")
}

pub fn init(db: Db, config: &DomainConfig, routing: FixedInstanceRoutingMap) -> anyhow::Result<()> {
    let supervisor = TasksSupervisor::new(db, config, routing)?;

    TASKS_SUPERVISOR.set(Supervisor::start(move |_| supervisor))
                    .map_err(|_| anyhow!("Tasks supervisor already initialized"))?;

    Ok(())
}

pub fn become_online() {
    get_tasks_supervisor().do_send(BecomeOnline);
}
