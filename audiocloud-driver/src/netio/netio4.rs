use actix::{Actor, Context, Handler, Recipient};
use serde::{Deserialize, Serialize};
use audiocloud_api::driver::InstanceDriverError;

use audiocloud_api::newtypes::FixedInstanceId;
use audiocloud_models::netio::netio_4c::*;
use audiocloud_models::Manufacturers::Netio;

use crate::{Command, InstanceConfig};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Config {
    pub instance: String,
    pub address:  String,
    pub username: String,
    pub password: String,
}

impl InstanceConfig for Config {
    fn instance_id(&self) -> FixedInstanceId {
        netio_power_pdu_4c_id().instance(self.instance.clone())
    }

    fn create(self, id: FixedInstanceId) -> anyhow::Result<Recipient<Command>> {
        Ok(Netio4c { id, config: self }.start().recipient())
    }
}

pub struct Netio4c {
    id:     FixedInstanceId,
    config: Config,
}

impl Actor for Netio4c {
    type Context = Context<Self>;
}

impl Handler<Command> for Netio4c {
    type Result = Result<(), InstanceDriverError>;

    fn handle(&mut self, msg: Command, ctx: &mut Self::Context) -> Self::Result {
        Ok(())
    }
}
