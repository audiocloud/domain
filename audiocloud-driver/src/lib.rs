use actix::{Message, Recipient};
use actix_broker::{Broker, SystemBroker};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use audiocloud_api::driver::{InstanceDriverCommand, InstanceDriverError, InstanceDriverEvent};
use audiocloud_api::newtypes::FixedInstanceId;

pub mod distopik;
pub mod http_client;
pub mod nats;
pub mod netio;
pub mod supervisor;
pub mod utils;
pub mod rest_api;

use tracing::*;

pub type ConfigFile = HashMap<FixedInstanceId, DriverConfig>;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DriverConfig {
    Distopik(distopik::Config),
    Netio(netio::Config),
}

impl InstanceConfig for DriverConfig {
    fn create(self, id: FixedInstanceId) -> anyhow::Result<Recipient<Command>> {
        match self {
            DriverConfig::Distopik(c) => c.create(id),
            DriverConfig::Netio(c) => c.create(id),
        }
    }
}

pub trait InstanceConfig {
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

pub fn emit_event(instance_id: FixedInstanceId, event: InstanceDriverEvent) {
    info!(id = %instance_id, "{}", serde_json::to_string(&event).unwrap());
    Broker::<SystemBroker>::issue_async(Event { instance_id, event });
}
