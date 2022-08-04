#![allow(unused_variables)]

use std::time::Duration;

use actix::{Actor, AsyncContext, Context, Handler, Recipient, Supervised};
use maplit::hashmap;
use serde::{Deserialize, Serialize};
use tracing::*;

use audiocloud_api::driver::{InstanceDriverCommand, InstanceDriverError, InstanceDriverEvent};
use audiocloud_api::model::{enumerate_multi_channel_value, enumerate_multi_channel_value_bool, ModelValue};
use audiocloud_api::newtypes::FixedInstanceId;
use audiocloud_api::time::Timestamped;
use audiocloud_models::netio::netio_4c::*;

use crate::{emit_event, Command, InstanceConfig};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Config;

impl InstanceConfig for Config {
    fn create(self, id: FixedInstanceId) -> anyhow::Result<Recipient<Command>> {
        info!(%id, config = ?self, "Creating instance");

        Ok(Netio4cMocked { id, state: [false; 4] }.start().recipient())
    }
}

struct Netio4cMocked {
    id:    FixedInstanceId,
    state: [bool; 4],
}

impl Netio4cMocked {
    pub fn emit(&self, evt: InstanceDriverEvent) {
        emit_event(self.id.clone(), evt);
    }
}

impl Actor for Netio4cMocked {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.restarting(ctx);
    }
}

impl Supervised for Netio4cMocked {
    fn restarting(&mut self, ctx: &mut <Self as Actor>::Context) {
        ctx.run_interval(Duration::from_secs(60), Self::update);
        self.update(ctx);
    }
}

impl Netio4cMocked {
    fn update(&mut self, ctx: &mut Context<Self>) {
        let power_values = self.state
                               .iter()
                               .cloned()
                               .map(ModelValue::Bool)
                               .map(Timestamped::from)
                               .enumerate()
                               .collect();

        self.emit(InstanceDriverEvent::Reports { reports: hashmap! { reports::POWER.clone() => power_values }, });
    }
}

impl Handler<Command> for Netio4cMocked {
    type Result = Result<(), InstanceDriverError>;

    fn handle(&mut self, msg: Command, ctx: &mut Context<Self>) -> Self::Result {
        match msg.command {
            InstanceDriverCommand::CheckConnection => self.emit(InstanceDriverEvent::Connected),
            InstanceDriverCommand::Stop
            | InstanceDriverCommand::Play { .. }
            | InstanceDriverCommand::Render { .. }
            | InstanceDriverCommand::Rewind { .. } => return Err(InstanceDriverError::MediaNotPresent),
            InstanceDriverCommand::SetParameters(mut p) => {
                if let Some(power) = p.remove(&params::POWER) {
                    for (i, value) in enumerate_multi_channel_value_bool(power) {
                        self.state[i] = value;
                    }
                }

                self.update(ctx);
            }
        }

        Ok(())
    }
}
