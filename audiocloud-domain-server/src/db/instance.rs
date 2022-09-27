use serde::{Deserialize, Serialize};

use audiocloud_api::common::media::PlayId;
use audiocloud_api::cloud::domains::{DomainMediaInstanceConfig, DomainPowerInstanceConfig};
use audiocloud_api::common::instance::{
    DesiredInstancePlayState, DesiredInstancePowerState, InstancePlayState, InstancePowerState,
};
use audiocloud_api::common::time::Timestamped;

use crate::tracker::RequestTracker;

#[derive(Serialize, Deserialize, Debug)]
pub struct InstancePower {
    pub spec: DomainPowerInstanceConfig,
    pub state:         Timestamped<InstancePowerState>,
    pub channel_state: Timestamped<bool>,
    pub desired:       Timestamped<DesiredInstancePowerState>,
    pub tracker:       RequestTracker,
}

impl From<DomainPowerInstanceConfig> for InstancePower {
    fn from(spec: DomainPowerInstanceConfig) -> Self {
        Self { spec,
               state: Timestamped::new(InstancePowerState::PoweredUp),
               desired: Timestamped::new(DesiredInstancePowerState::ShutDown),
               channel_state: Timestamped::new(false),
               tracker: Default::default() }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct InstancePlay {
    pub spec: DomainMediaInstanceConfig,
    pub state:   Timestamped<InstancePlayState>,
    pub media:   Option<Timestamped<f64>>,
    pub desired: Timestamped<DesiredInstancePlayState>,
    pub tracker: RequestTracker,
}

impl From<DomainMediaInstanceConfig> for InstancePlay {
    fn from(spec: DomainMediaInstanceConfig) -> Self {
        Self { spec,
               media: None,
               state: Timestamped::new(InstancePlayState::Playing { play_id: PlayId::new(u64::MAX), }),
               desired: Timestamped::new(DesiredInstancePlayState::Stopped),
               tracker: Default::default() }
    }
}
