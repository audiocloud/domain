use actix::{Addr, Supervisor};
use anyhow::anyhow;
use audiocloud_api::{SecureKey, SocketId};
use clap::Args;
use nanoid::nanoid;
pub use supervisor::SocketsSupervisor;
use tokio::sync::OnceCell;

mod messages;
mod supervisor;
mod web_rtc;
mod web_sockets;

static SOCKETS_SUPERVISOR: OnceCell<Addr<SocketsSupervisor>> = OnceCell::new();

#[derive(Args, Clone, Debug)]
pub struct SocketsOpts {
    #[clap(flatten)]
    web_rtc: web_rtc::WebRtcOpts,
}

fn get_next_socket_id() -> SocketId {
    SocketId::new(nanoid!())
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
struct SocketMembership {
    secure_key:    SecureKey,
    socket_id: SocketId,
}

pub fn init(cfg: SocketsOpts) -> anyhow::Result<()> {
    let supervisor = SocketsSupervisor::new(cfg);

    web_rtc::init(&cfg.web_rtc)?;

    SOCKETS_SUPERVISOR.set(Supervisor::start(move |_| supervisor))
                      .map_err(|_| anyhow!("Sockets supervisor already initialized"))?;

    Ok(())
}

pub fn get_sockets_supervisor() -> &'static Addr<SocketsSupervisor> {
    SOCKETS_SUPERVISOR.get().expect("Sockets supervisor not initialized")
}
