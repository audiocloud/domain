use audiocloud_api::cloud::domains::DomainMediaInstanceConfig;
use audiocloud_api::instance_driver::InstanceDriverCommand;
use audiocloud_api::{DesiredInstancePlayState, InstancePlayState, Timestamped};

use crate::tracker::RequestTracker;

pub struct Media {
    state:    Timestamped<InstancePlayState>,
    desired:  Timestamped<DesiredInstancePlayState>,
    config:   DomainMediaInstanceConfig,
    tracker:  RequestTracker,
    position: f64,
}

impl Media {
    pub fn new(config: DomainMediaInstanceConfig) -> Self {
        Self { state:    { Timestamped::new(InstancePlayState::Stopped) },
               desired:  { Timestamped::new(DesiredInstancePlayState::Stopped) },
               config:   { config },
               tracker:  { Default::default() },
               position: { 0.0 }, }
    }

    pub fn update(&mut self, send_command: impl FnOnce(InstanceDriverCommand)) {
        if !self.state.value().satisfies(self.desired.value()) {
            if self.tracker.should_retry() {
                send_command(self.desired.value().clone().into());
                self.tracker.retried();
            }
        }
    }

    pub fn set_desired_state(&mut self, desired: DesiredInstancePlayState) {
        if self.desired.value() != &desired {
            self.desired = Timestamped::new(desired);
        }
    }

    pub fn on_instance_play_state_changed(&mut self, state: InstancePlayState, position: Option<f64>) {
        self.state = state.into();
        if let Some(position) = position {
            self.position = position;
        }
    }
}
