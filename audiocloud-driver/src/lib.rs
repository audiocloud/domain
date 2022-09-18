use actix::{Message, Recipient};
use actix_broker::{Broker, SystemBroker};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::iter::repeat;

use audiocloud_api::instance_driver::{InstanceDriverCommand, InstanceDriverError, InstanceDriverEvent};
use audiocloud_api::common::model::{Model, MultiChannelValue};
use audiocloud_api::newtypes::{FixedInstanceId, ParameterId, ReportId};

pub mod distopik;
pub mod http_client;
pub mod nats;
pub mod netio;
pub mod rest_api;
pub mod supervisor;
pub mod utils;

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

#[derive(Message)]
#[rtype(result = "Result<NotifyInstanceValues, InstanceDriverError>")]
pub struct GetValues {
    pub instance_id: FixedInstanceId,
}

#[derive(Serialize, Deserialize, Clone, Debug, Message)]
#[rtype(result = "()")]
pub struct NotifyInstanceValues {
    pub instance_id: FixedInstanceId,
    pub parameters:  HashMap<ParameterId, MultiChannelValue>,
    pub reports:     HashMap<ReportId, MultiChannelValue>,
}

impl NotifyInstanceValues {
    pub fn new(instance_id: FixedInstanceId) -> Self {
        Self { instance_id,
               parameters: Default::default(),
               reports: Default::default() }
    }

    pub fn extend_parameters(&mut self, values: &HashMap<ParameterId, MultiChannelValue>, model: &Model) {
        for (key, values) in values {
            if let Some(parameter_meta) = model.parameters.get(key) {
                let dest = self.parameters
                               .entry(key.clone())
                               .or_insert_with(|| repeat(None).take(parameter_meta.scope.len(model)).collect());

                for (idx, value) in values.iter().enumerate() {
                    if let Some(dest) = dest.get_mut(idx) {
                        *dest = value.clone();
                    }
                }
            }
        }
    }
}

#[derive(Message)]
#[rtype(result = "HashSet<FixedInstanceId>")]
pub struct GetInstances;

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
