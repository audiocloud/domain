use std::collections::HashMap;

use actix::SystemService;

use audiocloud_api::cloud::apps::SessionSpec;
use audiocloud_api::instance::DesiredInstancePlayState;
use audiocloud_api::newtypes::FixedInstanceId;
use audiocloud_api::session::Session;

use crate::service::instance::{InstancesSupervisor, NotifyInstanceState, SetInstanceDesiredState};
use crate::tracker::RequestTracker;

pub struct SessionInstance {
    state:   NotifyInstanceState,
    tracker: RequestTracker,
}

impl SessionInstance {
    pub fn new(state_spec: NotifyInstanceState) -> Self {
        Self { state:   state_spec,
               tracker: RequestTracker::new(), }
    }

    pub fn reset_request_tracker(&mut self) {
        self.tracker.reset();
    }

    pub fn set_instance_state(&mut self, state: NotifyInstanceState) {
        self.state = state;
        self.reset_request_tracker();
    }

    pub fn update(&mut self, instance_id: &FixedInstanceId, play: &DesiredInstancePlayState) -> bool {
        return self.check_power() && self.check_play(instance_id, play);
    }

    pub fn check_power(&self) -> bool {
        if let Some(power) = &self.state.power {
            power.actual.value().is_powered_on()
        } else {
            true
        }
    }

    pub fn check_play(&mut self, instance_id: &FixedInstanceId, play: &DesiredInstancePlayState) -> bool {
        if let Some(media) = &self.state.play {
            if media.desired.value() != play {
                if self.tracker.should_retry() {
                    InstancesSupervisor::from_registry().do_send(SetInstanceDesiredState {
                        instance_id: instance_id.clone(),
                        desired: play.clone(),
                    });

                    self.tracker.retried();
                }

                false
            } else {
                media.actual.value().satisfies(play)
            }
        } else {
            true
        }
    }
}

pub struct SessionInstances {
    instances: HashMap<FixedInstanceId, SessionInstance>,
    play:      DesiredInstancePlayState,
}

impl Default for SessionInstances {
    fn default() -> Self {
        Self { instances: Default::default(),
               play:      DesiredInstancePlayState::Stopped, }
    }
}

impl SessionInstances {
    pub fn accept_instance_state(&mut self, notify: NotifyInstanceState) {
        let entry = self.instances
                        .entry(notify.instance_id.clone())
                        .or_insert_with(|| SessionInstance::new(notify.clone()));

        entry.set_instance_state(notify);
    }

    pub fn set_desired_state(&mut self, play: DesiredInstancePlayState) {
        self.play = play;
        for instance in self.instances.values_mut() {
            instance.reset_request_tracker();
        }
    }

    pub fn update(&mut self, session: &SessionSpec) -> bool {
        let mut rv = true;
        for instance_id in session.get_fixed_instance_ids() {
            if let Some(instance) = self.instances.get_mut(&instance_id) {
                rv &= instance.update(instance_id, &self.play);
            } else {
                rv = false;
            }
        }

        rv
    }
}
