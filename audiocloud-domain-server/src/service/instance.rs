#![allow(unused_variables)]

use actix::spawn;
use chrono::Duration;
use maplit::hashmap;
use rayon::prelude::*;
use tracing::*;

use audiocloud_api::driver::{InstanceDriverCommand, InstanceDriverEvent};
use audiocloud_api::instance::{power, DesiredInstancePowerState, InstancePowerState};
use audiocloud_api::model::ModelValue;
use audiocloud_api::newtypes::FixedInstanceId;
use audiocloud_api::session::InstanceParameters;
use audiocloud_api::time::Timestamped;

use crate::data::get_boot_cfg;
use crate::data::instance::{Instance, InstancePower};

pub mod actor;
