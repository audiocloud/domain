#![allow(unused_variables)]

use std::time::Duration;

use actix::{Actor, AsyncContext, Context, Handler, Recipient, Supervised};
use maplit::hashmap;
use serde::{Deserialize, Serialize};
use tracing::*;

use audiocloud_api::common::model::ModelValue;
use audiocloud_api::common::time::Timestamped;
use audiocloud_api::instance_driver::{InstanceDriverCommand, InstanceDriverError, InstanceDriverEvent};
use audiocloud_api::newtypes::FixedInstanceId;
use audiocloud_models::netio::PowerPdu4CReports;

use crate::driver::{Driver, DriverActor, Result};
use crate::{emit_event, Command, InstanceConfig};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Config;

impl InstanceConfig for Config {
    fn create(self, id: FixedInstanceId) -> anyhow::Result<Recipient<Command>> {
        info!(%id, config = ?self, "Creating instance");

        let driver = Netio4cMocked { id,
                                     state: vec![false; 4] };

        Ok(DriverActor::start_supervised_recipient(driver))
    }
}

struct Netio4cMocked {
    id:    FixedInstanceId,
    state: Vec<bool>,
}

impl Driver for Netio4cMocked {
    type Params = ();
    type Reports = PowerPdu4CReports;

    fn set_power_channel(&mut self, channel: usize, value: bool) -> Result {
        self.state[channel] = value;
        Ok(())
    }

    fn poll(&mut self) -> Option<Duration> {
        Self::emit_reports(self.id.clone(),
                           PowerPdu4CReports { power: Some(self.state.clone()),
                                               ..Default::default() });

        Some(Duration::from_secs(5))
    }
}
