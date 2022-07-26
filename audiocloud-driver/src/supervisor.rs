use std::collections::{HashMap, HashSet};

use actix::fut::LocalBoxActorFuture;
use actix::{fut, Actor, ActorFutureExt, Addr, Context, Handler, Recipient, Supervised, WrapFuture};
use anyhow::anyhow;
use audiocloud_api::driver::InstanceDriverError;
use once_cell::sync::OnceCell;

use audiocloud_api::newtypes::FixedInstanceId;

use crate::nats::NatsOpts;
use crate::{nats, Command, ConfigFile, InstanceConfig};

const SUPERVISOR_ADDR: OnceCell<Addr<DriverSupervisor>> = OnceCell::new();

pub fn get_driver_supervisor() -> Option<Addr<DriverSupervisor>> {
    SUPERVISOR_ADDR.get().cloned()
}

pub async fn init(nats_opts: NatsOpts, config: ConfigFile) -> anyhow::Result<()> {
    let supervisor = DriverSupervisor::new(nats_opts, config).await?;
    SUPERVISOR_ADDR.set(supervisor.start())
                   .map_err(|_| anyhow!("State init already called!"))?;
    Ok(())
}

pub struct DriverSupervisor {
    instances: HashMap<FixedInstanceId, Recipient<Command>>,
}

impl Handler<Command> for DriverSupervisor {
    type Result = LocalBoxActorFuture<Self, Result<(), InstanceDriverError>>;

    fn handle(&mut self, msg: Command, _ctx: &mut Context<Self>) -> Self::Result {
        let instance_id = msg.instance_id.clone();
        if let Some(instance) = self.instances.get(&instance_id) {
            instance.send(msg)
                    .into_actor(self)
                    .map(move |res, _, _| match res {
                        Err(_) => Err(InstanceDriverError::InstanceNotFound(instance_id)),
                        Ok(res) => res,
                    })
                    .boxed_local()
        } else {
            fut::err(InstanceDriverError::InstanceNotFound(msg.instance_id.clone())).into_actor(self)
                                                                                    .boxed_local()
        }
    }
}

impl DriverSupervisor {
    pub async fn new(nats_opts: NatsOpts, config: ConfigFile) -> anyhow::Result<Self> {
        let mut instances = HashMap::new();

        for (id, config) in config {
            let instance = config.create(id.clone())?;
            instances.insert(id, instance);
        }

        let instance_ids = instances.keys().cloned().collect::<HashSet<_>>();
        nats::init(nats_opts, instance_ids).await?;

        Ok(Self { instances })
    }
}

impl Actor for DriverSupervisor {
    type Context = Context<Self>;
}

impl Supervised for DriverSupervisor {}
