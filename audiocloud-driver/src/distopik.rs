use crate::{Command, InstanceConfig};
use actix::Recipient;
use audiocloud_api::newtypes::FixedInstanceId;
use serde::{Deserialize, Serialize};

#[cfg(unix)]
pub mod dual_1084;
pub mod summatra;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Config {
    #[cfg(unix)]
    Dual1084(dual_1084::Config),
    Summatra(summatra::Config),
}

impl InstanceConfig for Config {
    fn create(self, id: FixedInstanceId) -> anyhow::Result<Recipient<Command>> {
        match self {
            #[cfg(unix)]
            Config::Dual1084(c) => c.create(id),
            Config::Summatra(c) => c.create(id),
        }
    }
}
