#![allow(unused_variables)]

use std::time::Duration;

use actix::{
    spawn, Actor, ActorFutureExt, ArbiterHandle, AsyncContext, Context, Handler, Recipient, Running, Supervised,
    WrapFuture,
};
use futures::TryFutureExt;
use maplit::hashmap;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};
use tracing::*;

use audiocloud_api::common::model::ModelValue;
use audiocloud_api::common::time::{Timestamp, Timestamped};
use audiocloud_api::instance_driver::{InstanceDriverCommand, InstanceDriverError, InstanceDriverEvent};
use audiocloud_api::newtypes::FixedInstanceId;
use audiocloud_models::netio::netio_4c::*;

use crate::http_client::get_http_client;
use crate::{emit_event, Command, InstanceConfig};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Config {
    pub address: String,
    #[serde(default)]
    pub auth:    Option<(String, String)>,
}

impl InstanceConfig for Config {
    fn create(self, id: FixedInstanceId) -> anyhow::Result<Recipient<Command>> {
        info!(%id, config = ?self, "Creating instance");
        let base_url = Url::parse(&self.address)?;

        Ok(PowerPdu4c { id,
                        config: self,
                        base_url }.start()
                                  .recipient())
    }
}

pub struct PowerPdu4c {
    id:       FixedInstanceId,
    config:   Config,
    base_url: Url,
}

impl Actor for PowerPdu4c {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.restarting(ctx);
    }
}

impl Supervised for PowerPdu4c {
    fn restarting(&mut self, ctx: &mut <Self as Actor>::Context) {
        ctx.run_interval(Duration::from_secs(60), Self::update);
        self.update(ctx);
    }
}

impl Handler<Command> for PowerPdu4c {
    type Result = Result<(), InstanceDriverError>;

    fn handle(&mut self, msg: Command, ctx: &mut Self::Context) -> Self::Result {
        match msg.command {
            InstanceDriverCommand::CheckConnection => { /* makes no sense to check connection */ }
            InstanceDriverCommand::Stop
            | InstanceDriverCommand::Play { .. }
            | InstanceDriverCommand::Render { .. }
            | InstanceDriverCommand::Rewind { .. } => {
                return Err(InstanceDriverError::MediaNotPresent);
            }
            InstanceDriverCommand::SetParameters(mut params) => {
                if let Some(power) = params.remove(&params::POWER) {
                    let mut outputs = vec![];

                    for (i, desired_state) in power.into_iter().enumerate() {
                        if let Some(desired_state) = desired_state.and_then(ModelValue::into_bool) {
                            let desired_state = if desired_state {
                                PowerAction::On
                            } else {
                                PowerAction::Off
                            };

                            outputs.push(NetioPowerOutputAction { id:     (i + 1) as u32,
                                                                  action: desired_state, });
                        }
                    }

                    let netio_request = NetioPowerRequest { outputs };
                    let url = self.base_url.clone();
                    ctx.spawn(async move {
                                  let url = url.join("/netio.json")?;
                                  let response = get_http_client().post(url)
                                                                  .json(&netio_request)
                                                                  .send()
                                                                  .await?
                                                                  .json::<NetioPowerResponse>()
                                                                  .await?;

                                  anyhow::Result::<_>::Ok(response)
                              }.into_actor(self)
                               .map(|result, actor, _| match result {
                                   Err(err) => {
                                       emit_event(actor.id.clone(),
                                                  InstanceDriverEvent::IOError { error: err.to_string() });
                                   }
                                   Ok(response) => {
                                       Self::handle_response(actor.id.clone(), response);
                                   }
                               }));
                }
            }
        }
        Ok(())
    }
}

impl PowerPdu4c {
    fn update(&mut self, ctx: &mut Context<Self>) {
        let id = self.id.clone();
        let url = self.base_url.clone();
        spawn(async move {
                  let url = url.join("/netio.json")?;
                  let response = get_http_client().get(url)
                                                  .send()
                                                  .await?
                                                  .json::<NetioPowerResponse>()
                                                  .await?;

                  Self::handle_response(id, response);

                  anyhow::Result::<()>::Ok(())
              }.map_err(|err| {
                   info!(?err, "Update failed");
               }));
    }

    fn handle_response(id: FixedInstanceId, response: NetioPowerResponse) {
        let mut power_values = Vec::new();
        let mut current_values = Vec::new();

        for channel in response.outputs {
            let power_value = Timestamped::from(ModelValue::Bool(channel.state == PowerState::On));
            let current_value = Timestamped::from(ModelValue::Number(channel.current as f64 / 1000.0));
            let channel_id = (channel.id as usize) - 1;

            power_values.push(Some(power_value));
            current_values.push(Some(current_value));
        }

        let reports = hashmap! {
            reports::POWER.clone() => power_values,
            reports::CURRENT.clone() => current_values,
        };

        emit_event(id, InstanceDriverEvent::Reports { reports });
    }
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct NetioPowerRequest {
    pub outputs: Vec<NetioPowerOutputAction>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct NetioPowerOutputAction {
    #[serde(rename = "ID")]
    pub id:     u32,
    pub action: PowerAction,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct NetioPowerResponse {
    global_measure: NetioGlobalMeasure,
    outputs:        Vec<NetioPowerOutput>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct NetioGlobalMeasure {
    voltage:              f64,
    frequency:            f64,
    total_current:        u32,
    overall_power_factor: f64,
    total_load:           u32,
    total_energy:         u32,
    energy_start:         Timestamp,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct NetioPowerOutput {
    #[serde(rename = "ID")]
    id:           u32,
    name:         String,
    current:      u32,
    power_factor: f64,
    load:         u32,
    energy:       u32,
    state:        PowerState,
    action:       PowerAction,
}

#[derive(Serialize_repr, Deserialize_repr, Clone, Copy, Debug, PartialEq)]
#[repr(u32)]
enum PowerAction {
    Off = 0,
    On = 1,
    ShortOff = 2,
    ShortOn = 3,
    Toggle = 4,
    NoChange = 5,
    Ignore = 6,
}

#[derive(Serialize_repr, Deserialize_repr, Clone, Copy, Debug, PartialEq)]
#[repr(u32)]
enum PowerState {
    Off = 0,
    On = 1,
}
