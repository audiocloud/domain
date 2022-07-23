#![allow(unused_variables)]

use std::collections::HashMap;
use std::time::Duration;

use actix::{Actor, Addr, AsyncContext, Context, Handler, Message, Supervised, Supervisor, SystemService};
use actix_broker::{BrokerIssue, BrokerSubscribe};
use anyhow::anyhow;
use chrono::Utc;
use maplit::hashmap;
use tracing::*;

use audiocloud_api::cloud::domains::{BootDomain, DomainFixedInstance};
use audiocloud_api::driver::InstanceDriverEvent;
use audiocloud_api::instance::{
    DesiredInstancePlayState, InstancePlayState, InstancePowerState, ReportInstancePlayState, ReportInstancePowerState,
};
use audiocloud_api::model::ModelCapability::PowerDistributor;
use audiocloud_api::model::{multi_channel_value, Model, MultiChannelValue};
use audiocloud_api::newtypes::{AppSessionId, FixedInstanceId, ReportId};
use audiocloud_api::session::{InstanceParameters, InstanceReports};
use audiocloud_api::time::{Timestamp, Timestamped};

use crate::data::get_boot_cfg;
use crate::data::instance::{InstancePlay, InstancePower};
use crate::service::session::NotifySessionSpec;

pub fn init() {
    let _ = InstancesSupervisor::from_registry();
}

pub struct InstanceActor {
    id:               FixedInstanceId,
    power:            Option<InstancePower>,
    play:             Option<InstancePlay>,
    model:            Model,
    parameters:       InstanceParameters,
    reports:          InstanceReports,
    parameters_dirty: bool,
    owner:            Option<AppSessionId>,
    connected:        Timestamped<bool>,
    last_state_emit:  Option<Timestamp>,
}

impl Actor for InstanceActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.restarting(ctx);
    }
}

impl Supervised for InstanceActor {
    fn restarting(&mut self, ctx: &mut <Self as Actor>::Context) {
        ctx.run_interval(Duration::from_millis(30), Self::update);
        self.subscribe_system_async::<NotifySessionSpec>(ctx);
    }
}

impl InstanceActor {
    pub fn new(id_spec: FixedInstanceId, spec: DomainFixedInstance, model_spec: Model) -> Self {
        debug!(id = %id_spec, "Creating new instance actor");
        Self { id:               id_spec,
               owner:            None,
               last_state_emit:  None,
               connected:        Timestamped::new(false),
               power:            spec.power.map(InstancePower::from),
               play:             spec.media.map(InstancePlay::from),
               model:            model_spec,
               parameters:       Default::default(),
               reports:          Default::default(),
               parameters_dirty: false, }
    }

    fn update(&mut self, ctx: &mut Context<Self>) {
        self.update_power(ctx);

        self.update_play(ctx);

        if self.last_state_emit
               .as_ref()
               .map(|t| Utc::now() - *t > chrono::Duration::minutes(1))
               .unwrap_or(true)
        {
            self.emit_instance_state(ctx);
        }
    }

    fn update_play(&mut self, ctx: &mut Context<Self>) {
        if let Some(play) = &mut self.play {
            if !play.state.value().satisfies(play.desired.value()) {
                if play.tracker.should_retry() {
                    // TODO: send command to drivers
                    play.tracker.retried();
                }
            }
        }
    }

    fn update_power(&mut self, ctx: &mut Context<Self>) {
        if let Some(power) = &mut self.power {
            use InstancePowerState::*;
            let mut changed = false;

            match (*power.channel_state.value(), *power.state.value()) {
                (true, ShutDown | ShuttingDown) => {
                    power.state = PoweringUp.into();
                    changed = true;
                }
                (false, PoweredUp | PoweringUp) => {
                    power.state = ShuttingDown.into();
                    changed = true;
                }
                _ => {}
            }

            match (power.state.value(), power.state.elapsed()) {
                (PoweringUp, elapsed) if elapsed > chrono::Duration::milliseconds(power.spec.warm_up_ms as i64) => {
                    power.state = PoweredUp.into();
                    changed = true;
                }
                (ShuttingDown, elapsed) if elapsed > chrono::Duration::milliseconds(power.spec.cool_down_ms as i64) => {
                    power.state = ShutDown.into();
                    changed = true;
                }
                _ => {}
            }

            let channel_power_state = InstancePowerState::from_bool(*power.channel_state.value());

            if !channel_power_state.satisfies(*power.desired.value()) {
                if power.tracker.should_retry() {
                    use audiocloud_api::instance::power::params::POWER;
                    InstancesSupervisor::from_registry().do_send(
                        SetInstanceParameters {
                            instance_id: power.spec.instance.clone(),
                            parameters: hashmap! {
                                POWER.clone() => multi_channel_value::bool(power.spec.channel, power.desired.value().to_bool()),
                            }
                        },
                    );

                    power.tracker.retried();
                }
            }

            if changed {
                self.emit_instance_state(ctx);
            }
        }
    }

    fn set_connected(&mut self, connected: bool, ctx: &mut Context<Self>) {
        if self.connected.value() != &connected {
            self.connected = Timestamped::new(connected);
            self.emit_instance_state(ctx);
        }
    }

    fn set_reports(&mut self, reports: InstanceReports, _ctx: &mut Context<Self>) {
        for (report_id, report_value) in reports {
            self.reports
                .entry(report_id.clone())
                .or_default()
                .extend(report_value.into_iter());
        }

        self.issue_system_async(NotifyInstanceReports { instance_id: self.id.clone(),
                                                        reports:     self.reports.clone(), });

        if self.model.capabilities.contains(&PowerDistributor) {
            if let Some(power) = self.reports.get(&audiocloud_api::instance::power::reports::POWER) {
                if let Some(num_channels) = power.keys().max() {
                    let mut values = vec![false; *num_channels];
                    for (channel, value) in power.iter() {
                        values[*channel] = value.value().to_bool().unwrap_or_default();
                    }

                    self.issue_system_async(NotifyInstancePower { instance_id: self.id.clone(),
                                                                  power:       values, });
                }
            }
        }
    }

    fn set_play_state(&mut self, play_state: InstancePlayState, media: Option<f64>, ctx: &mut Context<Self>) {
        if let Some(play) = &mut self.play {
            let state_change = play.state.value() != &play_state;
            let media_change = play.media
                                   .as_ref()
                                   .map(|m| Some(m.value()) != media.as_ref())
                                   .unwrap_or_default();

            play.state = Timestamped::new(play_state);
            play.media = media.map(Timestamped::new);

            if state_change || media_change {
                self.emit_instance_state(ctx);
            }
        }
    }

    fn emit_instance_state(&mut self, ctx: &mut Context<Self>) {
        self.last_state_emit = Some(Utc::now());
        self.issue_system_async(NotifyInstanceState { instance_id: self.id.clone(),
                                                      power:       self.get_power_report(),
                                                      play:        self.get_play_report(),
                                                      connected:   self.connected.clone(), });
    }

    fn get_power_report(&self) -> Option<ReportInstancePowerState> {
        if let Some(power) = &self.power {
            Some(ReportInstancePowerState { actual:  power.state.clone(),
                                            desired: power.desired.clone(), })
        } else {
            None
        }
    }

    fn get_play_report(&self) -> Option<ReportInstancePlayState> {
        if let Some(play) = &self.play {
            Some(ReportInstancePlayState { actual:  play.state.clone(),
                                           desired: play.desired.clone(),
                                           media:   play.media.clone(), })
        } else {
            None
        }
    }
}

impl Handler<NotifySessionSpec> for InstanceActor {
    type Result = ();

    fn handle(&mut self, msg: NotifySessionSpec, ctx: &mut Self::Context) -> Self::Result {
        let mut changed = false;
        if self.owner.as_ref().map(|o| o == &msg.session_id).unwrap_or_default() {
            // incoming message is from our owner
            if msg.spec.fixed_instance_to_fixed_id(&self.id).is_none() {
                // we are now unlinked
                self.owner = None;
                changed = true;
            }
        } else {
            // incoming message is from someone else
            if msg.spec.fixed_instance_to_fixed_id(&self.id).is_some() {
                // but that someone is our new owner
                self.owner = Some(msg.session_id);
                changed = true;
            }
        }

        if changed {
            self.emit_instance_state(ctx);
        }
    }
}

impl Handler<SetInstanceParameters> for InstanceActor {
    type Result = ();

    fn handle(&mut self, msg: SetInstanceParameters, _ctx: &mut Self::Context) -> Self::Result {
        // TODO: merge parameters
        self.parameters_dirty = true;
    }
}

impl Handler<SetInstanceDesiredState> for InstanceActor {
    type Result = ();

    fn handle(&mut self, msg: SetInstanceDesiredState, _ctx: &mut Self::Context) -> Self::Result {
        if let Some(play) = &mut self.play {
            play.desired = Timestamped::new(msg.desired);
        }
    }
}

impl Handler<NotifyInstancePower> for InstanceActor {
    type Result = ();

    fn handle(&mut self, msg: NotifyInstancePower, _ctx: &mut Self::Context) -> Self::Result {
        if let Some(power) = &mut self.power {
            if &power.spec.instance == &msg.instance_id {
                match msg.power.get(power.spec.channel) {
                    Some(new_state) => {
                        power.channel_state = Timestamped::new(*new_state);
                    }
                    None => {
                        warn!(channel = power.spec.channel, "No power channel state received");
                    }
                }
            }
        }
    }
}

impl Handler<NotifyInstanceDriver> for InstanceActor {
    type Result = ();

    fn handle(&mut self, msg: NotifyInstanceDriver, ctx: &mut Self::Context) -> Self::Result {
        match msg.event {
            InstanceDriverEvent::Started => {
                self.set_connected(true, ctx);
            }
            InstanceDriverEvent::IOError { error } => {
                self.set_connected(false, ctx);
                self.issue_system_async(NotifyInstanceError { instance_id: self.id.clone(),
                                                              error:       error.clone(), });
            }
            InstanceDriverEvent::ConnectionLost => {
                self.set_connected(false, ctx);
            }
            InstanceDriverEvent::Connected => {
                self.set_connected(true, ctx);
            }
            InstanceDriverEvent::Reports { reports } => {
                self.set_connected(true, ctx);
                self.set_reports(reports, ctx);
            }
            InstanceDriverEvent::PlayState { desired,
                                             current,
                                             media, } => {
                self.set_connected(true, ctx);
                self.set_play_state(current, media, ctx);
            }
        }
    }
}

pub struct InstancesSupervisor {
    instances: HashMap<FixedInstanceId, Addr<InstanceActor>>,
}

impl InstancesSupervisor {
    pub fn new(boot: &BootDomain) -> anyhow::Result<Self> {
        let mut instances = HashMap::new();
        for (id, instance) in &boot.fixed_instances {
            let actor = InstanceActor::new(id.clone(),
                                           instance.clone(),
                                           boot.models
                                               .get(&id.model_id())
                                               .ok_or_else(|| anyhow!("Missing model"))?
                                               .clone());

            instances.insert(id.clone(), Supervisor::start(move |_| actor));
        }

        Ok(Self { instances })
    }
}

impl Actor for InstancesSupervisor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.restarting(ctx);
    }
}

impl Supervised for InstancesSupervisor {
    fn restarting(&mut self, ctx: &mut Context<Self>) {
        self.subscribe_system_async::<NotifyInstanceDriver>(ctx);
        self.subscribe_system_async::<NotifyInstancePower>(ctx);
    }
}

impl Handler<NotifyInstancePower> for InstancesSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyInstancePower, ctx: &mut Self::Context) -> Self::Result {
        if let Some(actor) = self.instances.get(&msg.instance_id) {
            actor.do_send(msg.clone())
        }
    }
}

impl Handler<SetInstanceParameters> for InstancesSupervisor {
    type Result = ();

    fn handle(&mut self, msg: SetInstanceParameters, ctx: &mut Context<InstancesSupervisor>) {
        if let Some(instance) = self.instances.get(&msg.instance_id) {
            instance.do_send(msg);
        }
    }
}

impl Handler<SetInstanceDesiredState> for InstancesSupervisor {
    type Result = ();

    fn handle(&mut self, msg: SetInstanceDesiredState, ctx: &mut Context<InstancesSupervisor>) {
        if let Some(instance) = self.instances.get(&msg.instance_id) {
            instance.do_send(msg);
        }
    }
}

impl Handler<NotifyInstanceDriver> for InstancesSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyInstanceDriver, ctx: &mut Self::Context) -> Self::Result {
        if let Some(instance) = self.instances.get(&msg.instance_id) {
            instance.do_send(msg);
        }
    }
}

impl Default for InstancesSupervisor {
    fn default() -> Self {
        Self::new(get_boot_cfg()).expect("Failed to create instances supervisor")
    }
}

impl SystemService for InstancesSupervisor {
    fn service_started(&mut self, ctx: &mut Context<Self>) {
        self.restarting(ctx);
    }
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct SetInstanceParameters {
    pub instance_id: FixedInstanceId,
    pub parameters:  InstanceParameters,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct SetInstanceDesiredState {
    pub instance_id: FixedInstanceId,
    pub desired:     DesiredInstancePlayState,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyInstanceReports {
    pub instance_id: FixedInstanceId,
    pub reports:     InstanceReports,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyInstanceDriver {
    pub instance_id: FixedInstanceId,
    pub event:       InstanceDriverEvent,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyInstancePower {
    pub instance_id: FixedInstanceId,
    pub power:       Vec<bool>,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyInstanceState {
    pub instance_id: FixedInstanceId,
    pub power:       Option<ReportInstancePowerState>,
    pub play:        Option<ReportInstancePlayState>,
    pub connected:   Timestamped<bool>,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyInstanceError {
    pub instance_id: FixedInstanceId,
    pub error:       String,
}
