use anyhow::anyhow;
use clap::Args;
use dashmap::DashMap;
use once_cell::sync::OnceCell;

use audiocloud_api::change::SessionState;
use audiocloud_api::cloud::domains::BootDomain;
use audiocloud_api::newtypes::{AppSessionId, FixedInstanceId, SocketId};
use audiocloud_api::time::Timestamped;

use crate::data::instance::{InstancePlay, InstancePower};

mod instance;
mod reaper;
mod session;

#[derive(Args)]
pub struct DataOpts {}

static STATE: OnceCell<DomainState> = OnceCell::new();

pub struct DomainState {
    pub instances:       DashMap<FixedInstanceId, instance::Instance>,
    pub sessions:        DashMap<AppSessionId, session::Session>,
    pub web_sockets:     DashMap<SocketId, ()>,
    pub web_rtc_sockets: DashMap<SocketId, ()>,
}

impl DomainState {
    async fn new(_: DataOpts, boot: BootDomain) -> anyhow::Result<Self> {
        let sessions = DashMap::new();
        for (id, session) in boot.sessions {
            sessions.insert(id,
                            session::Session { spec:   session.spec,
                                               state:  SessionState::default(),
                                               reaper: Timestamped::new(None), });
        }

        let instances = DashMap::new();
        for (id, instance) in boot.fixed_instances {
            instances.insert(id,
                             instance::Instance { play:  instance.media.map(InstancePlay::from),
                                                  power: instance.power.map(InstancePower::from), });
        }

        let web_sockets = DashMap::new();
        let web_rtc_sockets = DashMap::new();

        Ok(DomainState { instances,
                         sessions,
                         web_sockets,
                         web_rtc_sockets })
    }
}

pub async fn init(opts: DataOpts, boot: BootDomain) -> anyhow::Result<()> {
    let state = DomainState::new(opts, boot).await?;
    STATE.set(state).map_err(|_| anyhow!("State init already called!"))?;

    Ok(())
}
