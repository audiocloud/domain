use actix::{Message, Recipient};
use serde::{Deserialize, Serialize};

use audiocloud_api::driver::{InstanceDriverCommand, InstanceDriverError, InstanceDriverEvent};
use audiocloud_api::newtypes::FixedInstanceId;

pub mod distopik;
pub mod http_client;
pub mod nats;
pub mod netio;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ConfigFile {
    instances: Vec<DriverConfig>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DriverConfig {
    Distopik(distopik::Config),
    Netio(netio::Config),
}

impl InstanceConfig for DriverConfig {
    fn instance_id(&self) -> FixedInstanceId {
        match self {
            DriverConfig::Distopik(c) => c.instance_id(),
            DriverConfig::Netio(c) => c.instance_id(),
        }
    }

    fn create(self, id: FixedInstanceId) -> anyhow::Result<Recipient<Command>> {
        match self {
            DriverConfig::Distopik(c) => c.create(id),
            DriverConfig::Netio(c) => c.create(id),
        }
    }
}

pub trait InstanceConfig {
    fn instance_id(&self) -> FixedInstanceId;
    fn create(self, id: FixedInstanceId) -> anyhow::Result<Recipient<Command>>;
}

#[derive(Message)]
#[rtype(result = "Result<(), InstanceDriverError>")]
pub struct Command {
    pub instance_id: FixedInstanceId,
    pub command:     InstanceDriverCommand,
}

#[derive(Message, Clone)]
#[rtype(result = "()")]
pub struct Event {
    pub instance_id: FixedInstanceId,
    pub event:       InstanceDriverEvent,
}
