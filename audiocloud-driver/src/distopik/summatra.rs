use crate::{Command, InstanceConfig};
use actix::Recipient;
use audiocloud_api::newtypes::FixedInstanceId;
use audiocloud_models::distopik::summatra::distopik_summatra_id;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Config {
    instance: String,
}

impl InstanceConfig for Config {
    fn instance_id(&self) -> FixedInstanceId {
        distopik_summatra_id().instance(self.instance.clone())
    }

    fn create(self, id: FixedInstanceId) -> anyhow::Result<Recipient<Command>> {
        todo!()
    }
}
