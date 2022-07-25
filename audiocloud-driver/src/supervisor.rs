use std::collections::HashMap;

use actix::{Actor, Context, Recipient, Supervised};

use audiocloud_api::newtypes::FixedInstanceId;

use crate::{Command, ConfigFile, InstanceConfig};

pub struct DriverSupervisor {
    instances: HashMap<FixedInstanceId, Recipient<Command>>,
}

impl DriverSupervisor {
    pub fn new(config: ConfigFile) -> anyhow::Result<Self> {
        let mut instances = HashMap::new();

        for (id, config) in config {
            let instance = config.create(id.clone())?;
            instances.insert(id, instance);
        }

        Ok(Self { instances })
    }
}

impl Actor for DriverSupervisor {
    type Context = Context<Self>;
}

impl Supervised for DriverSupervisor {}
