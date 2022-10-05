#![allow(unused_variables)]

use std::collections::HashMap;
use std::time::Duration;

use actix::{
    Actor, ActorFutureExt, AsyncContext, Context, ContextFutureSpawner, Handler, MessageResult, Supervised, WrapFuture,
};
use actix_broker::{BrokerIssue, BrokerSubscribe};
use tracing::*;

use audiocloud_api::audio_engine::{EngineCommand, EngineError, EngineEvent};
use audiocloud_api::cloud::domains::FixedInstanceRouting;
use audiocloud_api::domain::tasks::TaskUpdated;
use audiocloud_api::{
    AppMediaObjectId, AppTaskId, DesiredInstancePlayState, DesiredTaskPlayState, DomainId, EngineId, FixedInstanceId,
    SerializableResult, Task, TaskReservation, TaskSecurity, TaskSpec,
};

use crate::config::NotifyFixedInstanceRouting;
use crate::fixed_instances::{
    get_instance_supervisor, GetMultipleFixedInstanceState, NotifyFixedInstanceReports, NotifyInstanceState,
};
use crate::nats;
use crate::tasks::task_engine::TaskEngine;
use crate::tasks::{
    NotifyEngineEvent, NotifyMediaTaskState, NotifyTaskActivated, NotifyTaskReservation, NotifyTaskSecurity,
    NotifyTaskSpec, SetTaskDesiredPlayState,
};

use super::task_fixed_instance::TaskFixedInstances;
use super::task_media_objects::TaskMediaObjects;

pub struct TaskActor {
    id:                     AppTaskId,
    engine_id:              EngineId,
    domain_id:              DomainId,
    reservations:           TaskReservation,
    spec:                   TaskSpec,
    security:               TaskSecurity,
    engine_command_subject: String,
    fixed_instance_routing: HashMap<FixedInstanceId, FixedInstanceRouting>,
    fixed_instances:        TaskFixedInstances,
    media_objects:          TaskMediaObjects,
    engine:                 TaskEngine,
}

impl Actor for TaskActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.restarting(ctx);
    }
}

impl Supervised for TaskActor {
    fn restarting(&mut self, ctx: &mut <Self as Actor>::Context) {
        self.notify_task_spec();

        self.notify_task_security();

        self.notify_task_reservation();

        self.issue_system_async(NotifyTaskActivated { task_id: Self.id.clone(), });

        // subscribe to routing changes
        self.subscribe_system_async::<NotifyFixedInstanceRouting>(ctx);

        // inform the engine that we want to start a task
        self.set_engine_spec(ctx);

        ctx.run_interval(Duration::from_millis(30), Self::update);
    }
}

impl Handler<NotifyFixedInstanceRouting> for TaskActor {
    type Result = ();

    fn handle(&mut self, msg: NotifyFixedInstanceRouting, ctx: &mut Self::Context) -> Self::Result {
        self.fixed_instance_routing = msg.routing;
    }
}

impl Handler<NotifyEngineEvent> for TaskActor {
    type Result = ();

    fn handle(&mut self, msg: NotifyEngineEvent, ctx: &mut Self::Context) -> Self::Result {
        if &self.engine_id != &msg.engine_id {
            return;
        }

        match msg.event {
            EngineEvent::Stopped { task_id } => {
                if &self.id == &task_id {
                    self.engine.set_actual_stopped();
                }
            }
            EngineEvent::Playing { task_id,
                                   play_id,
                                   audio,
                                   output_peak_meters,
                                   input_peak_meters,
                                   dynamic_reports, } => {
                if &self.id == &task_id {
                    self.engine.set_actual_playing(play_id);
                }
            }
            EngineEvent::PlayingFailed { task_id,
                                         play_id,
                                         error, } => {
                if &self.id == &task_id {
                    self.engine.set_desired_state(DesiredTaskPlayState::Stopped);
                    self.engine.set_actual_stopped();
                }
            }
            EngineEvent::Rendering { task_id,
                                     render_id,
                                     completion, } => {
                if &self.id == &task_id {
                    self.engine.set_actual_rendering(render_id);
                }
            }
            EngineEvent::RenderingFinished { task_id,
                                             render_id,
                                             path, } => {
                if &self.id == &task_id {
                    self.engine.set_desired_state(DesiredTaskPlayState::Stopped);
                    self.engine.set_actual_stopped();
                }
            }
            EngineEvent::RenderingFailed { task_id,
                                           render_id,
                                           error, } => {
                if &self.id == &task_id {
                    self.engine.set_desired_state(DesiredTaskPlayState::Stopped);
                    self.engine.set_actual_stopped();
                }
            }
            EngineEvent::Error { task_id, error } => {
                if &self.id == &task_id {
                    // do not modify desired states..
                }
            }
        }
    }
}

impl Handler<NotifyFixedInstanceReports> for TaskActor {
    type Result = ();

    fn handle(&mut self, msg: NotifyFixedInstanceReports, ctx: &mut Self::Context) -> Self::Result {
        // TODO: update the current streaming packet with report information
    }
}

impl Handler<NotifyMediaTaskState> for TaskActor {
    type Result = ();

    fn handle(&mut self, msg: NotifyMediaTaskState, ctx: &mut Self::Context) -> Self::Result {
        self.media_objects.update_media(msg.media);
        self.update(ctx);
    }
}

impl Handler<SetTaskDesiredPlayState> for TaskActor {
    type Result = MessageResult<SetTaskDesiredPlayState>;

    fn handle(&mut self, msg: SetTaskDesiredPlayState, ctx: &mut Self::Context) -> Self::Result {
        let desired_instance_state = match &msg.desired {
            DesiredTaskPlayState::Stopped => DesiredInstancePlayState::Stopped,
            DesiredTaskPlayState::Play(play) => DesiredInstancePlayState::Playing { play_id: play.play_id },
            DesiredTaskPlayState::Render(render) => DesiredInstancePlayState::Rendering { render_id: render.render_id,
                                                                                          length:    render.segment
                                                                                                           .length, },
        };

        self.fixed_instances.set_desired_state(desired_instance_state);
        let version = self.engine.set_desired_state(msg.desired);

        MessageResult(Ok(TaskUpdated::Updated { app_id:  { self.id.app_id.clone() },
                                                task_id: { self.id.task_id.clone() },
                                                version: { version }, }))
    }
}

impl TaskActor {
    pub fn new(id: AppTaskId,
               domain_id: DomainId,
               engine_id: EngineId,
               reservations: TaskReservation,
               spec: TaskSpec,
               security: TaskSecurity,
               routing: HashMap<FixedInstanceId, FixedInstanceRouting>)
               -> anyhow::Result<Self> {
        let engine_command_subject = engine_id.engine_command_subject();

        Ok(Self { id:                     { id.clone() },
                  engine_id:              { engine_id },
                  domain_id:              { domain_id },
                  reservations:           { reservations },
                  spec:                   { spec },
                  security:               { security },
                  engine_command_subject: { engine_command_subject },
                  fixed_instance_routing: { routing },
                  fixed_instances:        { TaskFixedInstances::default() },
                  media_objects:          { TaskMediaObjects::default() },
                  engine:                 { TaskEngine::new(id.clone()) }, })
    }

    fn update(&mut self, ctx: &mut <Self as Actor>::Context) {
        self.engine
            .set_instances_are_ready(self.fixed_instances.update(&self.spec));

        if let Some(engine_cmd) = self.engine.update() {
            nats::request_msgpack(self.engine_command_subject.clone(), engine_cmd).into_actor(self)
                                                                                  .map(Self::handle_engine_response)
                                                                                  .spawn(ctx)
        }
    }

    fn set_engine_spec(&mut self, ctx: &mut Context<TaskActor>) {
        let cmd = EngineCommand::SetSpec { task_id:     { self.id.clone() },
                                           spec:        { self.spec.clone() },
                                           instances:   { self.engine_fixed_instance_routing() },
                                           media_ready: { self.engine_media_paths() }, };

        nats::request_msgpack(self.engine_command_subject.clone(), cmd).into_actor(self)
                                                                       .map(Self::handle_engine_response)
                                                                       .spawn(ctx);
    }

    fn handle_engine_response(res: anyhow::Result<SerializableResult<(), EngineError>>,
                              actor: &mut Self,
                              ctx: &mut Context<Self>) {
        match res {
            Ok(SerializableResult::Error(error)) => {
                error!(%error, id = %actor.id, "Engine command failed");
            }
            Err(error) => {
                error!(%error, id = %actor.id, "Failed to deliver command to engine");
            }
            _ => {}
        }
    }

    fn update_fixed_instance_state(&self, ctx: &mut <Self as Actor>::Context) {
        get_instance_supervisor().send(GetMultipleFixedInstanceState { instance_ids: self.reservations
                                                                                         .fixed_instances
                                                                                         .clone(), })
                                 .into_actor(self)
                                 .map(|res, actor, ctx| {
                                     if let Ok(state) = res {
                                         actor.update_fixed_instance_state_inner(state, ctx);
                                     }
                                 })
                                 .spawn(ctx);
    }

    fn engine_fixed_instance_routing(&self) -> HashMap<FixedInstanceId, FixedInstanceRouting> {
        self.fixed_instance_routing
            .iter()
            .filter_map(|(id, routing)| {
                if self.reservations.fixed_instances.contains(id) {
                    Some((id.clone(), routing.clone()))
                } else {
                    None
                }
            })
            .collect()
    }

    fn engine_media_paths(&self) -> HashMap<AppMediaObjectId, String> {
        self.media_objects.ready_for_engine()
    }

    fn update_fixed_instance_state_inner(&mut self,
                                         result: HashMap<FixedInstanceId, NotifyInstanceState>,
                                         ctx: &mut <Self as Actor>::Context) {
        for (id, notify) in result {
            self.fixed_instances.notify_instance_state_changed(notify);
        }

        self.update(ctx);
    }

    fn notify_task_spec(&mut self) {
        self.issue_system_async(NotifyTaskSpec { task_id: Self.id.clone(),
                                                 spec:    self.spec.clone(), });
    }

    fn notify_task_security(&mut self) {
        self.issue_system_async(NotifyTaskSecurity { task_id:  self.id.clone(),
                                                     security: self.security.clone(), });
    }

    fn notify_task_reservation(&mut self) {
        self.issue_system_async(NotifyTaskReservation { task_id:     self.id.clone(),
                                                        reservation: self.reservations.clone(), });
    }
}
