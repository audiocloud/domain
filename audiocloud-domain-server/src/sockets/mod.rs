use actix::{Addr, Supervisor};
use anyhow::anyhow;
use clap::Args;
use nanoid::nanoid;
use once_cell::sync::OnceCell;

use audiocloud_api::{SecureKey, SocketId};
pub use messages::*;
pub use supervisor::SocketsSupervisor;
pub use web_sockets::configure;

mod messages;
mod supervisor;
mod web_rtc;
mod web_sockets;

static SOCKETS_SUPERVISOR: OnceCell<Addr<SocketsSupervisor>> = OnceCell::new();

#[derive(Args, Clone, Debug)]
pub struct SocketsOpts {
    #[clap(flatten)]
    web_rtc: web_rtc::WebRtcOpts,

    /// Milliseconds to keep streaming packets cached if for redelivery
    #[clap(long, env, default_value = "60000")]
    pub packet_cache_max_retention_ms: usize,
}

fn get_next_socket_id() -> SocketId {
    SocketId::new(nanoid!())
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
struct SocketMembership {
    secure_key: SecureKey,
    socket_id:  SocketId,
}

pub fn init(cfg: SocketsOpts) -> anyhow::Result<()> {
    let web_rtc_cfg = cfg.web_rtc.clone();
    let supervisor = SocketsSupervisor::new(cfg);

    web_rtc::init(&web_rtc_cfg)?;

    SOCKETS_SUPERVISOR.set(Supervisor::start(move |_| supervisor))
                      .map_err(|_| anyhow!("Sockets supervisor already initialized"))?;

    Ok(())
}

pub fn get_sockets_supervisor() -> &'static Addr<SocketsSupervisor> {
    SOCKETS_SUPERVISOR.get().expect("Sockets supervisor not initialized")
}
