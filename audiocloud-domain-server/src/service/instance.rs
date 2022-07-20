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

use crate::data::get_state;
use crate::data::instance::{InMemInstance, InMemInstancePower};

pub fn update_instances() {
    get_state().instances.par_iter_mut().for_each(|mut instance_with_id| {
                                            let id = instance_with_id.key().clone();
                                            update_instance(&id, instance_with_id.value_mut())
                                        });
}

fn update_instance_power(id: &FixedInstanceId, power: &mut InMemInstancePower) -> bool {
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
                                        power::params::POWER.clone() => hashmap! { power.spec.channel => power_state }
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
        for (parameter_id, values) in parameters {
            instance.parameters
                    .entry(parameter_id.clone())
                    .or_default()
                    .extend(values.into_iter());
        }

        instance.parameters_dirty = true;
    }
}

fn update_instance(id: &FixedInstanceId, instance: &mut InMemInstance) {
    let power_done = instance.power
                             .as_mut()
                             .map(|power| update_instance_power(id, power))
                             .unwrap_or(true);

    if instance.parameters_dirty {
        instance.parameters_dirty = false;
        spawn(send_instance_cmd(InstanceDriverCommand::SetParameters(instance.parameters.clone())));
    }
}

async fn send_instance_cmd(cmd: InstanceDriverCommand) {
    // TODO: send to nats
}

pub fn on_instance_evt(id: FixedInstanceId, evt: InstanceDriverEvent) {
    if let Some(mut instance) = get_state().instances.get_mut(&id) {
        match evt {
            InstanceDriverEvent::Started => {
                info!(%id, "instance started");
            }
            InstanceDriverEvent::IOError { error } => {
                error!(%error, %id, "IO error");
            }
            InstanceDriverEvent::ConnectionLost => {
                warn!(%id, "connection lost");
            }
            InstanceDriverEvent::Connected => {
                info!(%id, "connected");
            }
            InstanceDriverEvent::Reports { reports } => {
                for (report_id, report_value) in reports {
                    instance.reports
                            .entry(report_id.clone())
                            .or_default()
                            .extend(report_value.into_iter().map(|(k, v)| (k, v.into())));
                }
            }
            InstanceDriverEvent::PlayState { desired,
                                             current,
                                             media, } => {
                if let Some(play) = &mut instance.play {
                    if play.desired.value() == &desired {
                        play.state = current.into();
                        play.media = media.map(Timestamped::from);
                    }
                }
            }
        }
    }
}
