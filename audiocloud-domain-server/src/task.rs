#![allow(unused_variables)]

use std::mem;
use std::time::Duration;

use actix::{Actor, Addr, AsyncContext, Context, Handler, Supervised, SystemService};
use actix_broker::{BrokerIssue, BrokerSubscribe};
use chrono::Utc;

use audiocloud_api::audio_engine::command::AudioEngineCommand;
use audiocloud_api::common::change::{DesiredTaskPlayState, TaskPlayState, TaskState};
use audiocloud_api::common::instance::DesiredInstancePlayState;
use audiocloud_api::common::media::{PlayId, RenderId, RequestPlay, RequestRender};
use audiocloud_api::common::task::Task;
use audiocloud_api::common::time::Timestamped;
use audiocloud_api::domain::streaming::SessionPacketError;
use audiocloud_api::domain::tasks::TaskUpdated;
use audiocloud_api::newtypes::{AppTaskId, EngineId};
use audiocloud_api::StreamingPacket;
pub use messages::*;
use supervisor::SessionsSupervisor;

use crate::db::Db;
use crate::fixed_instances::{FixedInstancesSupervisor, NotifyInstanceError, NotifyInstanceReports, NotifyInstanceState};
use crate::task::messages::{NotifyMediaTaskState, NotifyStreamingPacket, NotifyTaskSecurity};
use crate::tracker::RequestTracker;
use crate::DomainResult;

pub mod messages;
pub mod session_instances;
pub mod session_media;
pub mod supervisor;

pub struct TaskActor {
    id:                   AppTaskId,
    task:                 Task,
    streaming_packet:     StreamingPacket,
    media:                session_media::SessionMedia,
    instances:            session_instances::SessionInstances,
    audio_engine:         EngineId,
    state:                TaskState,
    tracker:              RequestTracker,
    min_transmit_audio:   usize,
    instances_supervisor: Addr<FixedInstancesSupervisor>,
}

impl Actor for TaskActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.restarting(ctx);
    }
}

impl Supervised for TaskActor {
    fn restarting(&mut self, ctx: &mut Self::Context) {
        self.subscribe_system_async::<NotifyInstanceError>(ctx);
        self.subscribe_system_async::<NotifyInstanceReports>(ctx);
        self.subscribe_system_async::<NotifyInstanceState>(ctx);

        ctx.run_interval(Duration::from_millis(250), Self::update);

        self.emit_spec();
        self.emit_security();
    }
}

impl Handler<NotifyInstanceError> for TaskActor {
    type Result = ();

    fn handle(&mut self, msg: NotifyInstanceError, ctx: &mut Self::Context) -> Self::Result {
        match self.state.play_state.value() {
            TaskPlayState::Rendering(render) => {
                // TODO: cancel the render
            }
            _ => {}
        }
    }
}

impl Handler<NotifyInstanceReports> for TaskActor {
    type Result = ();

    fn handle(&mut self, msg: NotifyInstanceReports, ctx: &mut Self::Context) -> Self::Result {
        if let Some(fixed_id) = self.task.spec.fixed_instance_to_fixed_id(&msg.instance_id) {
            // push fixed instance reports to the media
        }
    }
}

impl Handler<NotifyInstanceState> for TaskActor {
    type Result = ();

    fn handle(&mut self, msg: NotifyInstanceState, ctx: &mut Self::Context) -> Self::Result {
        let should_update = self.task.spec.fixed_instance_to_fixed_id(&msg.instance_id).is_some();

        self.instances
            .notify_instance_state_changed(msg, &self.instances_supervisor);

        if should_update {
            self.update(ctx);
        }
    }
}

impl Handler<NotifyAudioEngineEvent> for TaskActor {
    type Result = ();

    fn handle(&mut self, msg: NotifyAudioEngineEvent, ctx: &mut Self::Context) -> Self::Result {
        use audiocloud_api::audio_engine::event::AudioEngineEvent::*;

        match msg.event {
            Stopped { task_id: session_id } => {
                if !self.state.play_state.value().is_stopped() {
                    self.state.play_state = TaskPlayState::Stopped.into();
                    self.emit_state();
                }
            }
            Playing { task_id: session_id,
                      play_id,
                      audio,
                      output_peak_meters,
                      input_peak_meters,
                      dynamic_reports, } => {
                if let DesiredTaskPlayState::Play(play) = self.state.desired_play_state.value() {
                    if play.play_id == play_id && !self.state.play_state.value().is_playing(play_id) {
                        self.state.play_state = TaskPlayState::Playing(play.clone()).into();
                        self.emit_state();
                    }
                }

                // TODO: add metering

                self.maybe_emit_packet();
            }
            Rendering { task_id: session_id,
                        render_id,
                        completion, } => {
                if let DesiredTaskPlayState::Render(render) = self.state.desired_play_state.value() {
                    if render.render_id == render_id && !self.state.play_state.value().is_rendering(render_id) {
                        self.state.play_state = TaskPlayState::Rendering(render.clone()).into();
                        self.emit_state();
                    }
                }
            }
            RenderingFinished { task_id: session_id,
                                render_id,
                                path, } => {
                if let TaskPlayState::Rendering(render) = self.state.play_state.value() {
                    if render.render_id == render_id {
                        self.issue_system_async(NotifyRenderComplete { render_id,
                                                                       path,
                                                                       task_id: self.id.clone(),
                                                                       object_id: render.object_id.clone(),
                                                                       put_url: render.put_url.clone(),
                                                                       notify_url: render.notify_url.clone(),
                                                                       context: render.context.to_string() });

                        self.state.play_state = TaskPlayState::Stopped.into();
                        self.emit_state();
                    }
                }

                self.set_stopped_state();
            }
            Error { task_id: session_id,
                    error, } => {
                self.streaming_packet
                    .errors
                    .push(Timestamped::new(SessionPacketError::General(error.to_string())));
            }
            PlayingFailed { task_id: session_id,
                            play_id,
                            error, } => {
                self.streaming_packet.add_play_error(play_id, error);
                self.set_stopped_state();
            }
            RenderingFailed { task_id: session_id,
                              render_id,
                              error: reason, } => {
                self.streaming_packet.add_render_error(render_id, reason);
                self.set_stopped_state();
            }
        }
    }
}

impl Handler<NotifyMediaTaskState> for TaskActor {
    type Result = ();

    fn handle(&mut self, msg: NotifyMediaTaskState, ctx: &mut Self::Context) -> Self::Result {
        let NotifyMediaTaskState { task_id: session_id,
                                   media, } = msg;
        if self.media.update_media(media) {
            let media_ready = self.media.ready_for_engine();
            let session_id = self.id.clone();

            self.audio_engine_request(ctx,
                                      AudioEngineCommand::Media { task_id: session_id,
                                                                  media_ready });
        }
    }
}

impl Handler<SetTaskDesiredState> for TaskActor {
    type Result = DomainResult<TaskUpdated>;

    fn handle(&mut self, msg: SetTaskDesiredState, ctx: &mut Self::Context) -> Self::Result {
        self.state.desired_play_state = msg.desired.into();
        self.update(ctx);

        Ok(TaskUpdated::Updated { app_id:  self.id.app_id.clone(),
                                  task_id: self.id.task_id.clone(),
                                  version: self.revision, })
    }
}

impl Handler<ExecuteTaskCommand> for TaskActor {
    type Result = anyhow::Result<()>;

    fn handle(&mut self, msg: ExecuteTaskCommand, ctx: &mut Self::Context) -> Self::Result {
        todo!()
    }
}

impl TaskActor {
    pub fn new(id: &AppTaskId, session: &Task, audio_engine_id: EngineId) -> Self {
        Self { id:                 id.clone(),
               task:               session.clone(),
               state:              Default::default(),
               media:              Default::default(),
               streaming_packet:   Default::default(),
               instances:          Default::default(),
               tracker:            Default::default(),
               audio_engine:       audio_engine_id,
               min_transmit_audio: 2, }
    }

    fn update(&mut self, ctx: &mut Context<Self>) {
        use DesiredTaskPlayState::*;
        use TaskPlayState::*;

        let mut modified = false;

        match (self.state.play_state.value(), self.state.desired_play_state.value()) {
            (TaskPlayState::Stopped, Play(play)) => {
                modified = true;

                self.tracker.reset();
                self.prepare_to_play(play.clone());
            }
            (TaskPlayState::Stopped, Render(render)) => {
                modified = true;

                self.tracker.reset();
                self.prepare_to_render(render.clone());
            }
            (Rendering(render), desired) if !desired.is_rendering_of(render) => {
                modified = true;

                let render_id = render.render_id.clone();
                self.prepare_render_stop(render_id);
                self.send_stop_render(ctx, render_id);
            }
            (Playing(play), desired) if !desired.is_playing_of(play) => {
                modified = true;
                let play_id = play.play_id.clone();

                self.prepare_play_stop(play_id);
                self.send_stop_play(ctx, play_id);
            }
            (PreparingToRender(prepare_render), Render(render)) if &prepare_render.render_id == &render.render_id => {
                if !self.instances.any_waiting() && !self.media.any_waiting() && self.tracker.should_retry() {
                    self.send_render(ctx, render.clone());
                }
            }
            (PreparingToPlay(prepare_play), Play(play)) if &prepare_play.play_id == &play.play_id => {
                if !self.instances.any_waiting() && self.tracker.should_retry() {
                    self.send_play(ctx, play.clone());
                }
            }
            (StoppingRender(render_id), _) => {
                if self.tracker.should_retry() {
                    self.send_stop_render(ctx, render_id.clone());
                }
            }
            (StoppingPlay(play_id), _) => {
                if self.tracker.should_retry() {
                    self.send_stop_play(ctx, play_id.clone());
                }
            }
            _ => {}
        }

        if modified {
            self.emit_state();
        }
    }

    fn send_stop_render(&mut self, ctx: &mut Context<TaskActor>, render_id: RenderId) {
        self.audio_engine_request(ctx,
                                  AudioEngineCommand::StopRender { task_id: self.id.clone(),
                                                                   render_id });
        self.tracker.retried();
    }

    fn send_stop_play(&mut self, ctx: &mut Context<TaskActor>, play_id: PlayId) {
        self.audio_engine_request(ctx,
                                  AudioEngineCommand::StopPlay { task_id: self.id.clone(),
                                                                 play_id });
        self.tracker.retried();
    }

    fn send_play(&mut self, ctx: &mut Context<TaskActor>, play: RequestPlay) {
        self.audio_engine_request(ctx,
                                  AudioEngineCommand::Play { task_id: self.id.clone(),
                                                             play });
        self.tracker.retried();
    }

    fn send_render(&mut self, ctx: &mut Context<TaskActor>, render: RequestRender) {
        self.audio_engine_request(ctx,
                                  AudioEngineCommand::Render { task_id: self.id.clone(),
                                                               render });
        self.tracker.retried();
    }

    fn prepare_to_play(&mut self, play: RequestPlay) {
        self.instances
            .set_desired_state(DesiredInstancePlayState::Playing { play_id: play.play_id.clone(), });

        self.state.play_state = TaskPlayState::PreparingToPlay(play).into();
    }

    fn prepare_to_render(&mut self, render: RequestRender) {
        self.instances
            .set_desired_state(DesiredInstancePlayState::Rendering { render_id: render.render_id.clone(),
                                                                     length:    render.segment.length, });

        self.state.play_state = TaskPlayState::PreparingToRender(render).into();
    }

    fn prepare_play_stop(&mut self, play_id: PlayId) {
        self.instances.set_desired_state(DesiredInstancePlayState::Stopped);
        self.state.play_state = TaskPlayState::StoppingPlay(play_id).into();
    }

    fn prepare_render_stop(&mut self, render_id: RenderId) {
        self.instances.set_desired_state(DesiredInstancePlayState::Stopped);
        self.state.play_state = TaskPlayState::StoppingRender(render_id).into();
    }

    fn handle_audio_engine_error(result: anyhow::Result<()>, actor: &mut Self, ctx: &mut Context<Self>) {
        if let Err(e) = result {
            actor.streaming_packet
                 .errors
                 .push(Timestamped::new(SessionPacketError::General(e.to_string())));
        }
    }

    fn emit_spec(&self) {
        self.issue_system_async(NotifyTaskSpec { task_id: self.id.clone(),
                                                 spec:    self.task.spec.clone(), });
    }

    fn emit_security(&self) {
        self.issue_system_async(NotifyTaskSecurity { session_id: self.id.clone(),
                                                     security:   self.task.security.clone(), });
    }

    fn emit_state(&self) {
        self.issue_system_async(NotifyTaskState { session_id: self.id.clone(),
                                                  state:      self.state.clone(), });
    }

    fn maybe_emit_packet(&mut self) {
        if (Utc::now() - self.streaming_packet.created_at) > chrono::Duration::milliseconds(500)
           || self.streaming_packet.compressed_audio.len() > self.min_transmit_audio
        {
            self.streaming_packet.play_state = self.state.play_state.value().clone();
            self.streaming_packet.desired_play_state = self.state.desired_play_state.value().clone();
            self.streaming_packet.waiting_for_instances = self.instances.waiting_for_instances();
            self.streaming_packet.waiting_for_media = self.media.waiting_for_media();

            let packet = mem::take(&mut self.streaming_packet);

            self.issue_system_async(NotifyStreamingPacket { session_id: self.id.clone(),
                                                            packet });
        }
    }

    fn set_stopped_state(&mut self) {
        self.state.play_state = TaskPlayState::Stopped.into();
        self.emit_state();
        self.maybe_emit_packet();
    }

    fn audio_engine_request(&self, ctx: &mut Context<Self>, request: AudioEngineCommand) {
        audio_engine::request(self.audio_engine.clone(), request).into_actor(self)
                                                                 .map(Self::handle_audio_engine_error)
                                                                 .wait(ctx);
    }
}

pub fn init(db: Db) {
    let _ = SessionsSupervisor::from_registry();
}

pub fn become_online() {
    SessionsSupervisor::from_registry().do_send(BecomeOnline);
}
