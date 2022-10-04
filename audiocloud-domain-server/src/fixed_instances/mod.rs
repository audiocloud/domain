use actix::{Addr, Supervisor};
use anyhow::anyhow;
use once_cell::sync::OnceCell;

use audiocloud_api::cloud::domains::{DomainConfig, FixedInstanceRoutingMap};
pub use messages::*;
pub use supervisor::FixedInstancesSupervisor;

use crate::db::Db;

mod instance;
mod media;
mod messages;
mod power;
mod supervisor;
mod values;

static INSTANCE_SUPERVISOR: OnceCell<Addr<FixedInstancesSupervisor>> = OnceCell::new();

pub fn get_instance_supervisor() -> &'static Addr<FixedInstancesSupervisor> {
    INSTANCE_SUPERVISOR.get().expect("Instance supervisor not initialized")
}

pub async fn init(cfg: &DomainConfig, db: Db) -> anyhow::Result<FixedInstanceRoutingMap> {
    let (routing, supervisor) = FixedInstancesSupervisor::new(cfg, db).await?;
    INSTANCE_SUPERVISOR.set(Supervisor::start(move |_| supervisor))
                       .map_err(|_| anyhow!("INSTANCE_SUPERVISOR already initialized"))?;

    Ok(routing)
}
