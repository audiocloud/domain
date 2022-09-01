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
use audiocloud_api::audio_engine::{AudioEngineCommand, AudioEngineEvent};
use audiocloud_api::change::{
    DesiredSessionPlayState, PlayId, PlaySession, RenderId, RenderSession, SessionPlayState, SessionState,
};
use audiocloud_api::instance::DesiredInstancePlayState;
use audiocloud_api::media::MediaServiceEvent;
use audiocloud_api::newtypes::{AppSessionId, AudioEngineId};
use audiocloud_api::session::Session;
use audiocloud_api::time::Timestamped;
use messages::{
    ExecuteSessionCommand, NotifyAudioEngineEvent, NotifyRenderComplete, NotifySessionSpec, NotifySessionState,
    SetSessionDesiredState,
};
use supervisor::{BecomeOnline, SessionsSupervisor};

use crate::audio_engine;
use crate::service::instance::{NotifyInstanceError, NotifyInstanceReports, NotifyInstanceState};
use crate::service::session::messages::{NotifyMediaServiceEvent, NotifySessionPacket};
use crate::tracker::RequestTracker;

pub mod messages;
pub mod session_instances;
pub mod session_media;
pub mod supervisor;

pub struct SessionActor {
    id:                 AppSessionId,
    session:            Session,
    packet:             SessionPacket,
    media:              session_media::SessionMedia,
    instances:          session_instances::SessionInstances,
    audio_engine:       AudioEngineId,
    state:              SessionState,
    tracker:            RequestTracker,
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
        use AudioEngineEvent::*;

        match msg.event {
            Stopped { session_id } => {
                if !self.state.play_state.value().is_stopped() {
                    self.state.play_state = SessionPlayState::Stopped.into();
                    self.emit_state();
                }
            }
            Playing { session_id,
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
            Rendering { session_id,
                        render_id,
                        completion, } => {
                if let DesiredSessionPlayState::Render(render) = self.state.desired_play_state.value() {
                    if render.render_id == render_id && !self.state.play_state.value().is_rendering(render_id) {
                        self.state.play_state = SessionPlayState::Rendering(render.clone()).into();
                        self.emit_state();
                    }
                }
            }
            RenderingFinished { session_id,
                                render_id,
                                path, } => {
                if let SessionPlayState::Rendering(render) = self.state.play_state.value() {
                    if render.render_id == render_id {
                        self.issue_system_async(NotifyRenderComplete { render_id,
                                                                       path,
                                                                       session_id: self.id.clone(),
                                                                       object_id: render.object_id.clone(),
                                                                       put_url: render.put_url.clone(),
                                                                       notify_url: render.notify_url.clone(),
                                                                       context: render.context.to_string() });

                        self.state.play_state = SessionPlayState::Stopped.into();
                        self.emit_state();
                    }
                }

                self.set_stopped_state();
            }
            Error { session_id, error } => {
                self.packet
                    .errors
                    .push(Timestamped::new(SessionPacketError::General(error.to_string())));
            }
            PlayingFailed { session_id,
                            play_id,
                            error, } => {
                self.packet.add_play_error(play_id, error);
                self.set_stopped_state();
            }
            RenderingFailed { session_id,
                              render_id,
                              reason, } => {
                self.packet.add_render_error(render_id, reason);
                self.set_stopped_state();
            }
        }
    }
}

impl Handler<NotifyMediaServiceEvent> for SessionActor {
    type Result = ();

    fn handle(&mut self, msg: NotifyMediaServiceEvent, ctx: &mut Self::Context) -> Self::Result {
        match msg.event {
            MediaServiceEvent::SessionMediaState { session_id, media } => {
                if self.media.update_media(media) {
                    let media_ready = self.media.ready_for_engine();
                    let session_id = self.id.clone();
                    self.audio_engine_request(ctx,
                                              AudioEngineCommand::Media { session_id,
                                                                          media_ready });
                }
            }
        }
    }
}

impl Handler<SetSessionDesiredState> for SessionActor {
    type Result = ();

    fn handle(&mut self, msg: SetSessionDesiredState, ctx: &mut Self::Context) -> Self::Result {
        self.state.desired_play_state = msg.desired.into();
        self.update(ctx);
    }
}

impl Handler<ExecuteSessionCommand> for SessionActor {
    type Result = anyhow::Result<()>;

    fn handle(&mut self, msg: ExecuteSessionCommand, ctx: &mut Self::Context) -> Self::Result {
        Err(anyhow!("Not implemented"))
    }
}

impl SessionActor {
    pub fn new(id: &AppSessionId, session: &Session, audio_engine_id: AudioEngineId) -> Self {
        Self { id:                 id.clone(),
               session:            session.clone(),
               state:              Default::default(),
               media:              Default::default(),
               packet:             Default::default(),
               instances:          Default::default(),
               tracker:            Default::default(),
               audio_engine:       audio_engine_id,
               min_transmit_audio: 2, }
    }

    fn update(&mut self, ctx: &mut Context<Self>) {
        use DesiredSessionPlayState::*;
        use SessionPlayState::*;

        let mut modified = false;

        match (self.state.play_state.value(), self.state.desired_play_state.value()) {
            (SessionPlayState::Stopped, Play(play)) => {
                modified = true;

                self.tracker.reset();
                self.prepare_to_play(play.clone());
            }
            (SessionPlayState::Stopped, Render(render)) => {
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

    fn send_stop_render(&mut self, ctx: &mut Context<SessionActor>, render_id: RenderId) {
        self.audio_engine_request(ctx,
                                  AudioEngineCommand::StopRender { session_id: self.id.clone(),
                                                                   render_id });
        self.tracker.retried();
    }

    fn send_stop_play(&mut self, ctx: &mut Context<SessionActor>, play_id: PlayId) {
        self.audio_engine_request(ctx,
                                  AudioEngineCommand::StopPlay { session_id: self.id.clone(),
                                                                 play_id });
        self.tracker.retried();
    }

    fn send_play(&mut self, ctx: &mut Context<SessionActor>, play: PlaySession) {
        self.audio_engine_request(ctx,
                                  AudioEngineCommand::Play { session_id: self.id.clone(),
                                                             play });
        self.tracker.retried();
    }

    fn send_render(&mut self, ctx: &mut Context<SessionActor>, render: RenderSession) {
        self.audio_engine_request(ctx,
                                  AudioEngineCommand::Render { session_id: self.id.clone(),
                                                               render });
        self.tracker.retried();
    }

    fn prepare_to_play(&mut self, play: PlaySession) {
        self.instances
            .set_desired_state(DesiredInstancePlayState::Playing { play_id: play.play_id.clone(), });

        self.state.play_state = SessionPlayState::PreparingToPlay(play).into();
    }

    fn prepare_to_render(&mut self, render: RenderSession) {
        self.instances
            .set_desired_state(DesiredInstancePlayState::Rendering { render_id: render.render_id.clone(),
                                                                     length:    render.segment.length, });

        self.state.play_state = SessionPlayState::PreparingToRender(render).into();
    }

    fn prepare_play_stop(&mut self, play_id: PlayId) {
        self.instances.set_desired_state(DesiredInstancePlayState::Stopped);
        self.state.play_state = SessionPlayState::StoppingPlay(play_id).into();
    }

    fn prepare_render_stop(&mut self, render_id: RenderId) {
        self.instances.set_desired_state(DesiredInstancePlayState::Stopped);
        self.state.play_state = SessionPlayState::StoppingRender(render_id).into();
    }

    fn handle_audio_engine_error(result: anyhow::Result<()>, actor: &mut Self, ctx: &mut Context<Self>) {
        if let Err(e) = result {
            actor.packet
                 .errors
                 .push(Timestamped::new(SessionPacketError::General(e.to_string())));
        }
    }

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

            let packet = mem::take(&mut self.packet);

            self.issue_system_async(NotifySessionPacket { session_id: self.id.clone(),
                                                          packet });
        }
    }

    fn set_stopped_state(&mut self) {
        self.state.play_state = SessionPlayState::Stopped.into();
        self.emit_state();
        self.maybe_emit_packet();
    }

    fn audio_engine_request(&self, ctx: &mut Context<Self>, request: AudioEngineCommand) {
        audio_engine::request(self.audio_engine.clone(), request).into_actor(self)
                                                                 .map(Self::handle_audio_engine_error)
                                                                 .wait(ctx);
    }
}

pub fn init() {
    let _ = SessionsSupervisor::from_registry();
}

pub fn become_online() {
    SessionsSupervisor::from_registry().do_send(BecomeOnline);
}
