#![allow(unused_variables)]

use std::mem;
use std::time::Duration;

use actix::{
    Actor, ActorFutureExt, AsyncContext, Context, ContextFutureSpawner, Handler, Supervised, SystemService, WrapFuture,
};
use actix_broker::{BrokerIssue, BrokerSubscribe};
use anyhow::anyhow;
use chrono::Utc;

use audiocloud_api::app::{SessionPacket, SessionPacketError};
use audiocloud_api::audio_engine::AudioEngineEvent;
use audiocloud_api::change::{DesiredSessionPlayState, PlayId, RenderId, SessionPlayState, SessionState};
use audiocloud_api::instance::DesiredInstancePlayState;
use audiocloud_api::newtypes::AppSessionId;
use audiocloud_api::session::{Session, SessionMode};
use audiocloud_api::time::Timestamped;
use messages::{
    ExecuteSessionCommand, NotifyAudioEngineEvent, NotifyRenderComplete, NotifySessionSpec, NotifySessionState,
    SetSessionDesiredState,
};
use supervisor::{BecomeOnline, SessionsSupervisor};

use crate::audio_engine::AudioEngineClient;
use crate::service::instance::{NotifyInstanceError, NotifyInstanceReports, NotifyInstanceState};
use crate::service::session::messages::NotifySessionPacket;

pub mod messages;
pub mod session_audio_engine;
pub mod session_instances;
pub mod session_media;
pub mod supervisor;

pub struct SessionActor {
    id:                 AppSessionId,
    session:            Session,
    packet:             SessionPacket,
    media:              session_media::SessionMedia,
    instances:          session_instances::SessionInstances,
    audio_engine:       session_audio_engine::SessionAudioEngineClient,
    state:              SessionState,
    min_transmit_audio: usize,
}

impl Actor for SessionActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.restarting(ctx);
    }
}

impl Supervised for SessionActor {
    fn restarting(&mut self, ctx: &mut Self::Context) {
        self.subscribe_system_async::<NotifyInstanceError>(ctx);
        self.subscribe_system_async::<NotifyInstanceReports>(ctx);
        self.subscribe_system_async::<NotifyInstanceState>(ctx);

        ctx.run_interval(Duration::from_millis(250), Self::update);

        self.emit_spec();
    }
}

impl Handler<NotifyInstanceError> for SessionActor {
    type Result = ();

    fn handle(&mut self, msg: NotifyInstanceError, ctx: &mut Self::Context) -> Self::Result {
        if let Some(fixed_id) = self.session.spec.fixed_instance_to_fixed_id(&msg.instance_id) {
            self.packet.push_fixed_error(fixed_id.clone(), msg.error);
        }
    }
}

impl Handler<NotifyInstanceReports> for SessionActor {
    type Result = ();

    fn handle(&mut self, msg: NotifyInstanceReports, ctx: &mut Self::Context) -> Self::Result {
        if let Some(fixed_id) = self.session.spec.fixed_instance_to_fixed_id(&msg.instance_id) {
            self.packet.push_fixed_instance_reports(fixed_id.clone(), msg.reports);
        }
    }
}

impl Handler<NotifyInstanceState> for SessionActor {
    type Result = ();

    fn handle(&mut self, msg: NotifyInstanceState, ctx: &mut Self::Context) -> Self::Result {
        let should_update = self.session.spec.fixed_instance_to_fixed_id(&msg.instance_id).is_some();

        self.instances.accept_instance_state(msg);

        if should_update {
            self.update(ctx);
        }
    }
}

impl Handler<NotifyAudioEngineEvent> for SessionActor {
    type Result = ();

    fn handle(&mut self, msg: NotifyAudioEngineEvent, ctx: &mut Self::Context) -> Self::Result {
        match msg.event {
            AudioEngineEvent::Loaded => {}
            AudioEngineEvent::Stopped { session_id } => {
                if !self.state.play_state.value().is_stopped() {
                    self.state.play_state = SessionPlayState::Stopped.into();
                    self.emit_state();
                }
            }
            AudioEngineEvent::Playing { session_id,
                                        play_id,
                                        audio,
                                        peak_meters,
                                        dynamic_reports, } => {
                if let DesiredSessionPlayState::Play(play) = self.state.desired_play_state.value() {
                    if play.play_id == play_id && !self.state.play_state.value().is_playing(play_id) {
                        self.state.play_state = SessionPlayState::Playing(play.clone()).into();
                        self.emit_state();
                    }
                }

                self.packet.push_peak_meters(peak_meters);
                self.packet.push_audio_packets(audio);
                self.maybe_emit_packet();
            }
            AudioEngineEvent::Rendering { session_id,
                                          render_id,
                                          completion, } => {
                if let DesiredSessionPlayState::Render(render) = self.state.desired_play_state.value() {
                    if render.render_id == render_id && !self.state.play_state.value().is_rendering(render_id) {
                        self.state.play_state = SessionPlayState::Rendering(render.clone()).into();
                        self.emit_state();
                    }
                }
            }
            AudioEngineEvent::RenderingFinished { session_id,
                                                  render_id,
                                                  path, } => {
                self.state.play_state = SessionPlayState::Stopped.into();
                self.issue_system_async(NotifyRenderComplete { session_id: self.id.clone(),
                                                               render_id,
                                                               path });
            }
            AudioEngineEvent::Error { session_id, error } => {
                self.packet
                    .errors
                    .push(Timestamped::new(SessionPacketError::General(error.to_string())));
            }
            AudioEngineEvent::PlayingFailed { session_id,
                                              play_id,
                                              error, } => {
                self.packet.add_play_error(play_id, error);
                self.state.play_state = SessionPlayState::Stopped.into();
                self.flush_packet();
                self.emit_state();
            }
            AudioEngineEvent::RenderingFailed { session_id,
                                                render_id,
                                                reason, } => {
                self.packet.add_render_error(render_id, reason);
                self.state.play_state = SessionPlayState::Stopped.into();
                self.flush_packet();
                self.emit_state();
            }
        }
    }
}

impl Handler<SetSessionDesiredState> for SessionActor {
    type Result = ();

    fn handle(&mut self, msg: SetSessionDesiredState, ctx: &mut Self::Context) -> Self::Result {
        if self.state.desired_play_state.value() != &msg.desired {
            self.state.desired_play_state = msg.desired.into();

            self.stop();
            self.update(ctx);
        }
    }
}

impl Handler<ExecuteSessionCommand> for SessionActor {
    type Result = anyhow::Result<()>;

    fn handle(&mut self, msg: ExecuteSessionCommand, ctx: &mut Self::Context) -> Self::Result {
        Err(anyhow!("Not implemented"))
    }
}

impl SessionActor {
    pub fn new(id: &AppSessionId, session: &Session, audio_engine: &AudioEngineClient) -> Self {
        Self { id:                 id.clone(),
               session:            session.clone(),
               state:              Default::default(),
               media:              Default::default(),
               packet:             Default::default(),
               instances:          Default::default(),
               audio_engine:       audio_engine.for_session(id.clone()),
               min_transmit_audio: 2, }
    }

    fn flush_packet(&mut self) {
        let packet = mem::take(&mut self.packet);
    }

    fn update(&mut self, ctx: &mut Context<Self>) {
        let mut modified = false;
        match (self.state.mode.value(), self.state.desired_play_state.value()) {
            (SessionMode::Idle, DesiredSessionPlayState::Play(play)) => {
                modified = true;
                self.prepare_to_play(play.play_id.clone());
                self.audio_engine
                    .clone()
                    .play(play.clone())
                    .into_actor(self)
                    .map(Self::handle_audio_engine_error)
                    .wait(ctx);
            }
            (SessionMode::Idle, DesiredSessionPlayState::Render(render)) => {
                modified = true;
                self.prepare_to_render(render.render_id.clone(), render.segment.length);
                self.audio_engine
                    .clone()
                    .render(render.clone())
                    .into_actor(self)
                    .map(Self::handle_audio_engine_error)
                    .wait(ctx);
            }
            (SessionMode::Rendering(render_id), desired) => {
                if !matches!(desired, DesiredSessionPlayState::Render(r) if render_id == &r.render_id) {
                    modified = true;
                    self.prepare_render_stop(render_id.clone());
                    self.audio_engine
                        .clone()
                        .stop()
                        .into_actor(self)
                        .map(Self::handle_audio_engine_error)
                        .wait(ctx);
                }
            }
            (SessionMode::Playing(play_id), desired) => {
                if !matches!(desired, DesiredSessionPlayState::Play(p) if play_id == &p.play_id) {
                    modified = true;
                    self.prepare_play_stop(play_id.clone());
                    self.audio_engine
                        .clone()
                        .stop()
                        .into_actor(self)
                        .map(Self::handle_audio_engine_error)
                        .wait(ctx);
                }
            }
            _ => {}
        }

        if modified {
            self.emit_state();
        }

        match self.state.mode.value() {
            SessionMode::StoppingRender(_) | SessionMode::StoppingPlay(_) => {
                self.update_stopping(ctx);
            }
            SessionMode::PreparingToPlay(_) | SessionMode::PreparingToRender(_) => {
                self.update_play_state(ctx);
            }
            SessionMode::Idle => {
                self.update_idle(ctx);
            }
            _ => {}
        }
    }

    fn prepare_to_play(&mut self, play_id: PlayId) {
        self.instances
            .set_desired_state(DesiredInstancePlayState::Playing { play_id: play_id.clone(), });

        self.state.mode = SessionMode::PreparingToPlay(play_id).into();
    }

    fn prepare_to_render(&mut self, render_id: RenderId, segment_length: f64) {
        self.instances
            .set_desired_state(DesiredInstancePlayState::Rendering { render_id: render_id.clone(),
                                                                     length:    segment_length, });

        self.state.mode = SessionMode::PreparingToRender(render_id).into();
    }

    fn prepare_play_stop(&mut self, play_id: PlayId) {
        self.instances.set_desired_state(DesiredInstancePlayState::Stopped);
        self.state.mode = SessionMode::StoppingPlay(play_id).into();
    }

    fn prepare_render_stop(&mut self, render_id: RenderId) {
        self.instances.set_desired_state(DesiredInstancePlayState::Stopped);
        self.state.mode = SessionMode::StoppingRender(render_id).into();
    }

    fn handle_audio_engine_error(result: anyhow::Result<()>, actor: &mut Self, ctx: &mut Context<Self>) {
        if let Err(e) = result {
            actor.packet
                 .errors
                 .push(Timestamped::new(SessionPacketError::General(e.to_string())));
        }
    }

    fn stop(&mut self) {
        match self.state.mode.value() {
            SessionMode::Playing(play_id) | SessionMode::PreparingToPlay(play_id) => {
                self.state.mode = SessionMode::StoppingPlay(play_id.clone()).into();
            }
            SessionMode::Rendering(render_id) | SessionMode::PreparingToRender(render_id) => {
                self.state.mode = SessionMode::StoppingRender(render_id.clone()).into();
            }
            _ => {}
        }
    }

    fn update_stopping(&mut self, ctx: &mut Context<Self>) {}

    fn set_idle(&mut self) {
        self.state.mode = SessionMode::Idle.into();
    }

    fn update_play_state(&mut self, ctx: &mut Context<Self>) {
        if !self.instances.update(&self.session.spec) {
            return;
        }

        // instances are fine, audio engine is also loaded
        if !self.state
                .play_state
                .value()
                .satisfies(self.state.desired_play_state.value())
        {
            // TODO: retry?
        } else {
            // update mode based on desired state
            self.state.mode =
                match self.state.desired_play_state.value() {
                    DesiredSessionPlayState::Play(play) => SessionMode::Playing(play.play_id.clone()),
                    DesiredSessionPlayState::Render(render) => SessionMode::Rendering(render.render_id.clone()),
                    DesiredSessionPlayState::Stopped => SessionMode::Idle,
                }.into();
        }
    }

    fn update_idle(&mut self, ctx: &mut Context<Self>) {}

    fn emit_spec(&self) {
        self.issue_system_async(NotifySessionSpec { session_id: self.id.clone(),
                                                    spec:       self.session.spec.clone(), });
    }

    fn emit_state(&self) {
        self.issue_system_async(NotifySessionState { session_id: self.id.clone(),
                                                     state:      self.state.clone(), });
    }

    fn maybe_emit_packet(&mut self) {
        if (Utc::now() - self.packet.created_at) > chrono::Duration::milliseconds(500)
           || self.packet.compressed_audio.len() > self.min_transmit_audio
        {
            self.packet.play_state = self.state.play_state.value().clone();
            self.packet.desired_play_state = self.state.desired_play_state.value().clone();
            self.packet.waiting_for_instances = self.instances.waiting_for_instances();
            self.packet.waiting_for_media = self.media.waiting_for_media();

            self.issue_system_async(NotifySessionPacket { session_id: self.id.clone(),
                                                          packet:     mem::take(&mut self.packet), });
        }
    }
}

pub fn init() {
    let _ = SessionsSupervisor::from_registry();
}

pub fn become_online() {
    SessionsSupervisor::from_registry().do_send(BecomeOnline);
}
