use serde::{Deserialize, Serialize};

use audiocloud_api::change::PlayId;
use audiocloud_api::cloud::domains::{DomainMediaInstanceSettings, DomainPowerInstanceSettings};
use audiocloud_api::instance::{
    DesiredInstancePlayState, DesiredInstancePowerState, InstancePlayState, InstancePowerState,
};
use audiocloud_api::model::Model;
use audiocloud_api::session::{InstanceParameters, InstanceReports};
use audiocloud_api::time::Timestamped;

use crate::tracker::RequestTracker;

#[derive(Serialize, Deserialize, Debug)]
pub struct Instance {
    pub power:            Option<InstancePower>,
    pub play:             Option<InstancePlay>,
    pub model:            Model,
    pub parameters:       InstanceParameters,
    pub reports:          InstanceReports,
    pub parameters_dirty: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct InstancePower {
    pub spec:    DomainPowerInstanceSettings,
    pub state:   Timestamped<InstancePowerState>,
    pub desired: Timestamped<DesiredInstancePowerState>,
    pub tracker: RequestTracker,
}

impl From<DomainPowerInstanceSettings> for InstancePower {
    fn from(spec: DomainPowerInstanceSettings) -> Self {
        Self { spec,
               state: Timestamped::new(InstancePowerState::PoweredUp),
               desired: Timestamped::new(DesiredInstancePowerState::ShutDown),
               tracker: Default::default() }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct InstancePlay {
    pub spec:    DomainMediaInstanceSettings,
    pub state:   Timestamped<InstancePlayState>,
    pub media:   Option<Timestamped<f64>>,
    pub desired: Timestamped<DesiredInstancePlayState>,
    pub tracker: RequestTracker,
}

impl From<DomainMediaInstanceSettings> for InstancePlay {
    fn from(spec: DomainMediaInstanceSettings) -> Self {
        Self { spec,
               media: None,
               state: Timestamped::new(InstancePlayState::Playing { play_id: PlayId::new(u64::MAX), }),
               desired: Timestamped::new(DesiredInstancePlayState::Stopped),
               tracker: Default::default() }
    }
}
