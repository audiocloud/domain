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
use audiocloud_models::netio::PowerPdu4CReports;

use crate::driver::Result;
use crate::driver::{Driver, DriverActor};
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

        Ok(DriverActor::start_supervised_recipient(PowerPdu4c { id,
                                                                config: self,
                                                                base_url }))
    }
}

pub struct PowerPdu4c {
    id:       FixedInstanceId,
    config:   Config,
    base_url: Url,
}

impl Driver for PowerPdu4c {
    type Params = ();
    type Reports = ();

    fn set_power_channel(&mut self, channel: usize, value: bool) -> Result {
        let id = self.id.clone();
        let url = self.base_url.clone();

        // TODO: wtf we don't actually update it ?!

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

        Ok(())
    }
}

impl PowerPdu4c {
    fn handle_response(id: FixedInstanceId, response: NetioPowerResponse) {
        let mut power_values = vec![false; 4];
        let mut current_values = vec![0.0; 4];
        let mut reports = PowerPdu4CReports::default();

        for channel in response.outputs {
            let power_value = channel.state == PowerState::On;
            let current_value = channel.current as f64 / 1000.0;
            let channel_id = (channel.id as usize) - 1;

            power_values[channel_id] = power_value;
            current_values[channel_id] = current_value;
        }

        reports.power = Some(power_values);
        reports.current = Some(current_values);

        if let Ok(reports) = serde_json::to_value(&reports) {
            emit_event(id, InstanceDriverEvent::Reports { reports });
        }
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
pub struct NetioPowerResponse {
    global_measure: NetioGlobalMeasure,
    outputs:        Vec<NetioPowerOutput>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct NetioGlobalMeasure {
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
pub struct NetioPowerOutput {
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
pub enum PowerAction {
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
pub enum PowerState {
    Off = 0,
    On = 1,
}
