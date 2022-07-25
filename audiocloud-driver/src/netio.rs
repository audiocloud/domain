use crate::{Command, InstanceConfig};
use actix::{Actor, Recipient};
use audiocloud_api::newtypes::FixedInstanceId;
use serde::{Deserialize, Serialize};

pub mod power_pdu_4c;
pub mod power_pdu_4c_mocked;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Config {
    #[serde(rename = "power_pdu_4c")]
    PowerPdu4c(power_pdu_4c::Config),

    #[serde(rename = "power_pdu_4c_mocked")]
    PowerPdu4cMocked(power_pdu_4c_mocked::Config),
}

impl InstanceConfig for Config {
    fn create(self, id: FixedInstanceId) -> anyhow::Result<Recipient<Command>> {
        match self {
            Config::PowerPdu4c(c) => c.create(id),
            Config::PowerPdu4cMocked(c) => c.create(id),
        }
    }
}
