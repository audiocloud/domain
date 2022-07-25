use actix::Recipient;
use serde::{Deserialize, Serialize};

use audiocloud_api::newtypes::FixedInstanceId;
use audiocloud_models::distopik::dual_1084::distopik_dual_1084_id;

use crate::{Command, InstanceConfig};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Config;

impl InstanceConfig for Config {
    fn create(self, id: FixedInstanceId) -> anyhow::Result<Recipient<Command>> {
        todo!()
    }
}
