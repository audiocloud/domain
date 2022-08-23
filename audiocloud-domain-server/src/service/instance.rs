#![allow(unused_variables)]

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use actix::{
    Actor, ActorFutureExt, Addr, AsyncContext, Context, ContextFutureSpawner, Handler, Message, Supervised, Supervisor,
    SystemService, WrapFuture,
};
use actix_broker::{BrokerIssue, BrokerSubscribe};
use anyhow::anyhow;
use chrono::Utc;
use maplit::hashmap;
use tracing::*;

use audiocloud_api::cloud::domains::{BootDomain, DomainFixedInstance};
use audiocloud_api::driver::{InstanceDriverCommand, InstanceDriverEvent};
use audiocloud_api::instance::{
    power, DesiredInstancePlayState, InstancePlayState, InstancePowerState, ReportInstancePlayState,
    ReportInstancePowerState,
};
use audiocloud_api::model::ModelCapability::PowerDistributor;
use audiocloud_api::model::{multi_channel_value, Model};
use audiocloud_api::newtypes::{AppSessionId, FixedInstanceId, ParameterId};
use audiocloud_api::session::{InstanceParameters, InstanceReports};
use audiocloud_api::time::{Timestamp, Timestamped};

use crate::data::get_boot_cfg;
use crate::data::instance::{InstancePlay, InstancePower};
use crate::instance;
use crate::service::session::messages::NotifySessionSpec;

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
    parameters_dirty: HashSet<ParameterId>,
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
    #[instrument(skip_all)]
    pub fn new(id_spec: FixedInstanceId, spec: DomainFixedInstance, model_spec: Model) -> Self {
        debug!(id = %id_spec, "Creating new instance actor");

        let reports = model_spec.default_reports();

        Self { id:               id_spec,
               owner:            None,
               last_state_emit:  None,
               connected:        Timestamped::new(false),
               power:            spec.power.map(InstancePower::from),
               play:             spec.media.map(InstancePlay::from),
               model:            model_spec,
               parameters:       Default::default(),
               reports:          Default::default(),
               parameters_dirty: Default::default(), }
    }

    #[instrument(skip_all)]
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

        if !self.parameters_dirty.is_empty() {
            self.send_dirty_parameters(ctx);
        }
    }

    fn handle_instance_driver_error(result: anyhow::Result<()>, actor: &mut Self, ctx: &mut Context<Self>) {
        if let Err(e) = result {
            // TODO: anything more meaningful? retries?
            warn!(%e, "Error on instance driver request");
        }
    }

    #[instrument(skip_all)]
    fn send_dirty_parameters(&mut self, ctx: &mut Context<InstanceActor>) {
        let mut rv = HashMap::new();
        for id in self.parameters_dirty.drain() {
            if let Some(parameter) = self.parameters.get(&id) {
                rv.insert(id, parameter.clone());
            }
        }

        if !rv.is_empty() {
            let cmd = InstanceDriverCommand::SetParameters(rv);
            instance::request(self.id.clone(), cmd).into_actor(self)
                                                   .map(Self::handle_instance_driver_error)
                                                   .wait(ctx);
        }
    }

    #[instrument(skip_all)]
    fn update_play(&mut self, ctx: &mut Context<Self>) {
        if let Some(play) = &mut self.play {
            if !play.state.value().satisfies(play.desired.value()) {
                if play.tracker.should_retry() {
                    let cmd = match play.desired.value() {
                        DesiredInstancePlayState::Playing { play_id } => {
                            InstanceDriverCommand::Play { play_id: play_id.clone(), }
                        }
                        DesiredInstancePlayState::Rendering { length, render_id } => {
                            InstanceDriverCommand::Render { length:    *length,
                                                            render_id: render_id.clone(), }
                        }
                        DesiredInstancePlayState::Stopped => InstanceDriverCommand::Stop,
                    };

                    play.tracker.retried();

                    instance::request(self.id.clone(), cmd).into_actor(self)
                                                           .map(Self::handle_instance_driver_error)
                                                           .wait(ctx);
                }
            }
        }
    }

    #[instrument(skip_all)]
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

    #[instrument(skip_all)]
    fn set_connected(&mut self, connected: bool, ctx: &mut Context<Self>) {
        if self.connected.value() != &connected {
            self.connected = Timestamped::new(connected);
            self.emit_instance_state(ctx);
        }
    }

    #[instrument(skip_all)]
    fn set_reports(&mut self, reports: InstanceReports, _ctx: &mut Context<Self>) {
        let mut overwritten = HashSet::new();

        for (report_id, report_value) in reports {
            let report = self.reports.entry(report_id.clone()).or_default();
            let is_volatile = self.model
                                  .reports
                                  .get(&report_id)
                                  .map(|r| r.volatile)
                                  .unwrap_or_default();

            let mut overwritten_report = false;
            for (channel, value) in report_value.into_iter().enumerate() {
                if let (Some(target), Some(new_value)) = (report.get_mut(channel), value) {
                    let modified = target.as_ref().map(|t| t.value() != new_value.value()).unwrap_or(true);
                    if is_volatile || modified {
                        overwritten_report = true;
                        *target = Some(new_value);
                    }
                }
            }

            if overwritten_report {
                overwritten.insert(report_id.clone());
            }
        }

        if overwritten.is_empty() {
            return;
        }

        let mut reports_to_issue = HashMap::new();
        for report_id in &overwritten {
            if let Some(report) = self.reports.get(report_id) {
                reports_to_issue.insert(report_id.clone(), report.clone());
            }
        }

        self.issue_system_async(NotifyInstanceReports { instance_id: self.id.clone(),
                                                        reports:     reports_to_issue, });

        let maybe_power_report = self.reports.get(&power::reports::POWER);
        let is_power_controller = self.model.capabilities.contains(&PowerDistributor);
        let power_changed = overwritten.contains(&power::reports::POWER);

        if let (Some(power), true, true) = (maybe_power_report, power_changed, is_power_controller) {
            let values = power.iter()
                              .map(|v| v.as_ref().and_then(|v| v.value().to_bool()).unwrap_or_default())
                              .collect();

            self.issue_system_async(NotifyInstancePower { instance_id: self.id.clone(),
                                                          power:       values, });
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

    fn handle(&mut self, msg: SetInstanceParameters, ctx: &mut Self::Context) -> Self::Result {
        for (parameter_id, parameter_multi_value) in msg.parameters {
            if let Some(current_multi_value) = self.parameters.get_mut(&parameter_id) {
                let mut dirty = false;
                let max_count = parameter_multi_value.len();
                if max_count > current_multi_value.len() {
                    current_multi_value.resize(max_count, None);
                    dirty = true;
                }

                for (index, maybe_new_value) in parameter_multi_value.into_iter().enumerate() {
                    if let Some(new_value) = maybe_new_value {
                        if current_multi_value[index].as_ref()
                                                     .map(|v| v != &new_value)
                                                     .unwrap_or(true)
                        {
                            dirty = true;
                            current_multi_value[index] = Some(new_value);
                        }
                    }
                }

                if dirty {
                    self.parameters_dirty.insert(parameter_id.clone());
                }
            }
        }

        self.send_dirty_parameters(ctx);
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
