use actix::{Actor, Addr, Handler, Supervised, Supervisor, SystemService};
use anyhow::anyhow;
use clap::Args;
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

pub fn init(db: Db, opts: &TaskOpts, config: &DomainConfig, routing: FixedInstanceRoutingMap) -> anyhow::Result<()> {
    let supervisor = TasksSupervisor::new(db, opts, config, routing)?;

    TASKS_SUPERVISOR.set(Supervisor::start(move |_| supervisor))
                    .map_err(|_| anyhow!("Tasks supervisor already initialized"))?;

    Ok(())
}

pub fn become_online() {
    get_tasks_supervisor().do_send(BecomeOnline);
}

#[derive(Args, Clone, Debug, Copy)]
pub struct TaskOpts {
    /// Number of seconds to keep task information in the supervisor before forgetting it
    #[clap(long, env, default_value = "3600")]
    pub task_grace_seconds: usize,

    /// Send streaming packets to clients as soon as they exceed specified age in milliseconds (even if no audio captured)
    #[clap(long, env, default_value = "250")]
    pub max_packet_age_ms: usize,

    /// Send streaming packets to clients as soon as they exceed specified count of compressed audio buffers (even if not old enough)
    #[clap(long, env, default_value = "4")]
    pub max_packet_audio_frames: usize,
}
