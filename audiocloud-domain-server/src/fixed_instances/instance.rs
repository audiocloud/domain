#![allow(unused_variables)]

use std::time::Duration;

use actix::{
    Actor, ActorFutureExt, AsyncContext, Context, ContextFutureSpawner, Handler, StreamHandler, Supervised, WrapFuture,
};
use futures::FutureExt;
use tracing::warn;

use audiocloud_api::{
    FixedInstanceId, InstancePlayState, InstanceReports, PowerDistributorReports, Model, ModelCapability,
    Request, SerializableResult, Timestamped,
};
use audiocloud_api::cloud::domains::DomainFixedInstanceConfig;
use audiocloud_api::instance_driver::{InstanceDriverCommand, InstanceDriverEvent};

use crate::fixed_instances::{
    get_instance_supervisor, NotifyInstancePowerChannelsChanged, NotifyInstanceReports, SetInstanceParameters,
};
use crate::fixed_instances::values::merge_values;
use crate::nats;
use crate::task::{NotifyTaskDeleted, NotifyTaskSpec};

use super::media::Media;
use super::power::Power;

pub struct InstanceActor {
    id:                  FixedInstanceId,
    config:              DomainFixedInstanceConfig,
    power:               Option<Power>,
    media:               Option<Media>,
    spec:                Timestamped<Option<NotifyTaskSpec>>,
    parameters:          serde_json::Value,
    instance_driver_cmd: String,
    model:               Model,
}

impl InstanceActor {
    pub fn new(id: FixedInstanceId, config: DomainFixedInstanceConfig, model: Model) -> anyhow::Result<Self> {
        let power = config.power.clone().map(Power::new);
        let media = config.media.clone().map(Media::new);
        let instance_driver_cmd = id.driver_command_subject();

        Ok(Self { id:                  { id },
                  config:              { config },
                  power:               { power },
                  media:               { media },
                  model:               { model },
                  spec:                { Default::default() },
                  parameters:          { Default::default() },
                  instance_driver_cmd: { instance_driver_cmd }, })
    }

    fn on_instance_driver_reports(&mut self, reports: InstanceReports) {
        if self.model.capabilities.contains(&ModelCapability::PowerDistributor) {
            match serde_json::from_value::<PowerDistributorReports>(reports.clone()) {
                Ok(reports) => {
                    if let Some(power_channels) = reports.power {
                        get_instance_supervisor().do_send(NotifyInstancePowerChannelsChanged { instance_id:
                                                                                                   self.id.clone(),
                                                                                               power:
                                                                                                   power_channels, });
                    }
                }
                Err(_) => {}
            }
        }

        get_instance_supervisor().do_send(NotifyInstanceReports { instance_id: self.id.clone(),
                                                                  reports });
    }

    fn on_instance_driver_connected(&mut self, ctx: &mut Context<InstanceActor>) {
        // set current parameters
        self.request_instance_driver(InstanceDriverCommand::SetParameters(self.parameters.clone()), ctx);
    }

    fn on_instance_driver_play_state_changed(&mut self, current: InstancePlayState, media_pos: Option<f64>) {
        if let Some(media) = self.media.as_mut() {
            media.on_instance_play_state_changed(current, media_pos);
        }
    }
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
        ctx.add_stream(nats::subscribe_json::<InstanceDriverEvent>(self.id.driver_event_subject()));
    }
}

impl Handler<NotifyTaskSpec> for InstanceActor {
    type Result = ();

    fn handle(&mut self, msg: NotifyTaskSpec, ctx: &mut Self::Context) -> Self::Result {
        if msg.spec.get_fixed_instance_ids().contains(&self.id) {
            self.spec = Some(msg).into();
            self.update(ctx);
        }
    }
}

impl Handler<NotifyTaskDeleted> for InstanceActor {
    type Result = ();

    fn handle(&mut self, msg: NotifyTaskDeleted, ctx: &mut Self::Context) -> Self::Result {
        if self.spec
               .value()
               .map(|prev_notify| &prev_notify.task_id == &msg.task_id)
           == Some(true)
        {
            self.spec = None.into();
        }
    }
}

impl Handler<SetInstanceParameters> for InstanceActor {
    type Result = ();

    fn handle(&mut self, msg: SetInstanceParameters, ctx: &mut Self::Context) -> Self::Result {
        merge_values(&mut self.parameters, msg.parameters);
    }
}

impl StreamHandler<InstanceDriverEvent> for InstanceActor {
    fn handle(&mut self, item: InstanceDriverEvent, ctx: &mut Self::Context) {
        match item {
            InstanceDriverEvent::Started => {}
            InstanceDriverEvent::IOError { .. } => {}
            InstanceDriverEvent::ConnectionLost => {}
            InstanceDriverEvent::Connected => {
                self.on_instance_driver_connected(ctx);
            }
            InstanceDriverEvent::Reports { reports } => {
                self.on_instance_driver_reports(reports);
            }
            InstanceDriverEvent::PlayState { desired,
                                             current,
                                             media: media_pos, } => {
                self.on_instance_driver_play_state_changed(current, media_pos)
            }
        }
    }

    fn finished(&mut self, ctx: &mut Self::Context) {
        self.restarting(ctx);
    }
}

impl InstanceActor {
    fn update(&mut self, ctx: &mut Context<Self>) {
        if let Some(power) = self.power.as_mut() {
            power.update(&self.spec, |set_params| {
                     get_instance_supervisor().send(set_params)
                                              .map(drop)
                                              .into_actor(self)
                                              .spawn(ctx)
                 });
        }
        if let Some(media) = self.media.as_mut() {
            media.update(|cmd| self.request_instance_driver(cmd, ctx));
        }
    }

    fn request_instance_driver(&self, driver: InstanceDriverCommand, ctx: &mut <Self as Actor>::Context) {
        nats::request_json(&self.instance_driver_cmd, driver).into_actor(self)
                                                             .map(Self::on_instance_driver_response)
                                                             .spawn(ctx);
    }

    fn on_instance_driver_response(response: anyhow::Result<<InstanceDriverCommand as Request>::Response>,
                                   actor: &mut Self,
                                   ctx: &mut <Self as Actor>::Context) {
        let instance = &actor.id;
        match response {
            Ok(result) => match result {
                SerializableResult::Ok(_) => {}
                SerializableResult::Error(error) => {
                    warn!(%instance, %error, "Instance driver responded with error");
                }
            },
            Err(error) => {
                warn!(%instance, %error, "Failed to send command to instance driver");
            }
        }
    }
}
