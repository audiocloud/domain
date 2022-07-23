use actix::{Actor, Context, Handler, Recipient, SystemService};
use actix_broker::BrokerIssue;
use maplit::hashmap;

use audiocloud_api::driver::{InstanceDriverCommand, InstanceDriverError, InstanceDriverEvent};
use audiocloud_api::instance::power;
use audiocloud_api::model::ModelValue;
use audiocloud_api::newtypes::FixedInstanceId;
use audiocloud_api::time::Timestamped;
use audiocloud_models::netio::netio_4c::*;
use serde::{Deserialize, Serialize};

use crate::nats::{NatsService, Publish};
use crate::{emit_event, Command, Event, InstanceConfig};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Config {
    instance: String,
}

impl InstanceConfig for Config {
    fn instance_id(&self) -> FixedInstanceId {
        netio_power_pdu_4c_id().instance(self.instance.clone())
    }

    fn create(self, id: FixedInstanceId) -> anyhow::Result<Recipient<Command>> {
        Ok(Netio4cMocked { id,
                           config: self,
                           state: [false; 4] }.start()
                                              .recipient())
    }
}

struct Netio4cMocked {
    id:     FixedInstanceId,
    config: Config,
    state:  [bool; 4],
}

impl Netio4cMocked {
    pub fn emit(&self, evt: InstanceDriverEvent) {
        emit_event(self.id.clone(), evt);
    }
}

impl Actor for Netio4cMocked {
    type Context = Context<Self>;
}

impl Handler<Command> for Netio4cMocked {
    type Result = Result<(), InstanceDriverError>;

    fn handle(&mut self, msg: Command, _ctx: &mut Context<Self>) -> Self::Result {
        match msg.command {
            InstanceDriverCommand::CheckConnection => self.emit(InstanceDriverEvent::Connected),
            InstanceDriverCommand::Stop
            | InstanceDriverCommand::Play { .. }
            | InstanceDriverCommand::Render { .. }
            | InstanceDriverCommand::Rewind { .. } => return Err(InstanceDriverError::MediaNotPresent),
            InstanceDriverCommand::SetParameters(mut p) => {
                if let Some(power) = p.remove(&power::params::POWER) {
                    for (i, value) in power {
                        if let Some(value) = value.to_bool() {
                            self.state[i] = value;
                        }
                    }
                }

                let power_values = self.state
                                       .iter()
                                       .cloned()
                                       .map(ModelValue::Bool)
                                       .map(Timestamped::from)
                                       .enumerate()
                                       .collect();
                
                self.emit(InstanceDriverEvent::Reports { reports: hashmap! {
                                                             power::reports::POWER.clone() => power_values,
                                                         }, });
            }
        }

        Ok(())
    }
}
