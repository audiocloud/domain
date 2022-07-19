#![allow(unused_variables)]

use actix::spawn;
use chrono::Duration;
use maplit::hashmap;
use rayon::prelude::*;

use audiocloud_api::driver::InstanceDriverCommand;
use audiocloud_api::instance::{power, DesiredInstancePowerState, InstancePowerState};
use audiocloud_api::model::ModelValue;
use audiocloud_api::newtypes::FixedInstanceId;
use audiocloud_api::session::InstanceParameters;

use crate::data::get_state;
use crate::data::instance::{Instance, InstancePower};

pub fn update_instances() {
    get_state().instances.par_iter_mut().for_each(|mut instance_with_id| {
                                            let id = instance_with_id.key().clone();
                                            update_instance(&id, instance_with_id.value_mut())
                                        });
}

fn update_instance_power(id: &FixedInstanceId, power: &mut InstancePower) -> bool {
    // are we in a transitional state
    match power.state.value() {
        InstancePowerState::WarmingUp => {
            if power.state.elapsed() > Duration::milliseconds(power.spec.warm_up_ms as i64) {
                power.state = InstancePowerState::PoweredUp.into();
            }
        }
        InstancePowerState::CoolingDown => {
            if power.state.elapsed() > Duration::milliseconds(power.spec.cool_down_ms as i64) {
                power.state = InstancePowerState::ShutDown.into()
            }
        }
        _ => {}
    }

    if !power.state.value().satisfies(*power.desired.value()) {
        if power.tracker.should_retry() {
            power.tracker.retried();
            let power_state = match power.desired.value() {
                DesiredInstancePowerState::PoweredUp => ModelValue::Bool(true),
                DesiredInstancePowerState::ShutDown => ModelValue::Bool(false),
            };

            set_instance_parameters(&power.spec.instance,
                                    hashmap! {
                                        power::params::POWER.clone() => vec![(power.spec.channel, power_state)],
                                    });
            false
        } else {
            false
        }
    } else {
        true
    }
}

fn set_instance_parameters(id: &FixedInstanceId, parameters: InstanceParameters) {
    if let Some(mut instance) = get_state().instances.get_mut(id) {
        for (set_id, set_value) in parameters {
            for (param_id, mut value) in &mut instance.parameters {
                if param_id == &set_id {
                    for (set_ch, set_value) in &set_value {
                        let mut found = false;
                        for current in value.iter_mut() {
                            if &current.0 == set_ch {
                                current.1 = set_value.clone();
                                found = true;
                                break;
                            }
                        }

                        if !found {
                            value.push((*set_ch, set_value.clone()));
                        }
                    }

                    value.sort_by(|a, b| a.0.cmp(&b.0));
                }
            }
        }

        instance.parameters_dirty = true;
    }
}

fn update_instance(id: &FixedInstanceId, instance: &mut Instance) {
    let power_done = instance.power
                             .as_mut()
                             .map(|power| update_instance_power(id, power))
                             .unwrap_or(true);

    if instance.parameters_dirty {
        spawn(send_instance_cmd(InstanceDriverCommand::SetParameters(instance.parameters.clone())));
        instance.parameters_dirty = false;
    }
}

async fn send_instance_cmd(cmd: InstanceDriverCommand) {
    // TODO: send to nats
}
