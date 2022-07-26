use std::collections::{HashMap, HashSet};

use actix::{Actor, Context, Recipient, Supervised};

use audiocloud_api::newtypes::FixedInstanceId;

use crate::nats::NatsOpts;
use crate::{nats, Command, ConfigFile, InstanceConfig};

pub struct DriverSupervisor {
    instances: HashMap<FixedInstanceId, Recipient<Command>>,
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
