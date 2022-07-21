use crate::{Command, InstanceConfig};
use actix::{Actor, Recipient};
use audiocloud_api::newtypes::FixedInstanceId;
use serde::{Deserialize, Serialize};

pub mod netio4;
pub mod netio4_mocked;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Config {
    #[serde(rename = "power_pdu_4c")]
    PowerPdu4c(netio4::Config),
    #[serde(rename = "power_pdu_4c_mocked")]
    PowerPdu4cMocked(netio4_mocked::Config),
}

impl InstanceConfig for Config {
    fn instance_id(&self) -> FixedInstanceId {
        match self {
            Config::PowerPdu4c(c) => c.instance_id(),
            Config::PowerPdu4cMocked(c) => c.instance_id(),
        }
    }

    fn create(self, id: FixedInstanceId) -> anyhow::Result<Recipient<Command>> {
        match self {
            Config::PowerPdu4c(c) => c.create(id),
            Config::PowerPdu4cMocked(c) => c.create(id),
        }
    }
}
