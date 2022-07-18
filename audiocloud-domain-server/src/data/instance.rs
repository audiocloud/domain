use serde::{Deserialize, Serialize};

use audiocloud_api::change::PlayId;
use audiocloud_api::cloud::domains::{DomainMediaInstanceSettings, DomainPowerInstanceSettings};
use audiocloud_api::instance::{
    DesiredInstancePlayState, DesiredInstancePowerState, InstancePlayState, InstancePowerState,
};
use audiocloud_api::newtypes::FixedInstanceId;
use audiocloud_api::time::Timestamped;

#[derive(Serialize, Deserialize, Debug)]
pub struct Instance {
    pub _id:   FixedInstanceId,
    pub power: Option<InstancePower>,
    pub play:  Option<InstancePlay>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct InstancePower {
    pub spec:    DomainPowerInstanceSettings,
    pub state:   Timestamped<InstancePowerState>,
    pub desired: Timestamped<DesiredInstancePowerState>,
}

impl From<DomainPowerInstanceSettings> for InstancePower {
    fn from(spec: DomainPowerInstanceSettings) -> Self {
        Self { spec,
               state: Timestamped::new(InstancePowerState::PoweredUp),
               desired: Timestamped::new(DesiredInstancePowerState::ShutDown) }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct InstancePlay {
    pub spec:    DomainMediaInstanceSettings,
    pub state:   Timestamped<InstancePlayState>,
    pub desired: Timestamped<DesiredInstancePlayState>,
}

impl From<DomainMediaInstanceSettings> for InstancePlay {
    fn from(spec: DomainMediaInstanceSettings) -> Self {
        Self { spec,
               state: Timestamped::new(InstancePlayState::Playing { play_id: PlayId::new(u64::MAX), }),
               desired: Timestamped::new(DesiredInstancePlayState::Stopped) }
    }
}
