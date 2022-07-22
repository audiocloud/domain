use std::collections::HashMap;
use std::mem;
use std::time::Duration;

use actix::{Actor, Addr, ArbiterHandle, AsyncContext, Context, Handler, Message, Running, Supervised};
use actix_broker::{BrokerIssue, BrokerSubscribe};

use audiocloud_api::app::SessionPacket;
use audiocloud_api::audio_engine::{AudioEngineCommand, AudioEngineEvent};
use audiocloud_api::change::{PlayId, RenderId, SessionPlayState, SessionState};
use audiocloud_api::cloud::apps::SessionSpec;
use audiocloud_api::newtypes::{AppSessionId, FixedInstanceId};
use audiocloud_api::session::SessionMode;

use crate::data::session::Session;
use crate::service::instance::{NotifyInstanceError, NotifyInstanceReports, NotifyInstanceState};
use crate::tracker::RequestTracker;

pub struct SessionActor {
    id:                   AppSessionId,
    session:              Session,
    packet:               SessionPacket,
    instance_states:      HashMap<FixedInstanceId, NotifyInstanceState>,
    state:                SessionState,
    audio_engine_tracker: RequestTracker,
    engine_loaded:        bool,
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
        self.instance_states.insert(msg.instance_id.clone(), msg);
        if self.session.spec.fixed_instance_to_fixed_id(&msg.instance_id).is_some() {
            self.update(ctx);
        }
    }
}

impl Handler<NotifyAudioEngineEvent> for SessionActor {
    type Result = ();

    fn handle(&mut self, msg: NotifyAudioEngineEvent, ctx: &mut Self::Context) -> Self::Result {
        match msg.event {
            AudioEngineEvent::Loaded => {
                self.engine_loaded = false;
            }
            AudioEngineEvent::Stopped => {
                if !self.state.play_state.value().is_stopped() {
                    self.state.play_state = SessionPlayState::Stopped.into();
                    self.emit_state();
                }
            }
            AudioEngineEvent::Playing { playing, audio } => {
                if !self.state.play_state.value().is_playing(playing.play_id) {
                    self.state.play_state = SessionPlayState::Playing(playing).into();
                    self.emit_state();
                }
                self.packet.push_audio_engine_playing(audio);
            }
            AudioEngineEvent::Rendering { rendering } => {
                if !self.state.play_state.value().is_rendering(rendering.render_id) {
                    self.state.play_state = SessionPlayState::Rendering(rendering).into();
                    self.emit_state();
                }
            }
            AudioEngineEvent::RenderingFinished { render_id, path } => {
                self.state.play_state = SessionPlayState::Stopped.into();
                self.issue_system_async(NotifyRenderComplete { session_id: self.id.clone(),
                                                               render_id,
                                                               path });
            }
            AudioEngineEvent::Meters { peak_meters } => {
                self.packet.push_audio_engine_meters(peak_meters);
            }
            AudioEngineEvent::Exit { .. } => {
                self.engine_loaded = false;
            }
        }
    }
}

impl SessionActor {
    fn update(&mut self, ctx: &mut Context<Self>) {
        match self.state.mode.value() {
            SessionMode::StoppingRender(_) => {
                self.stopping_render(ctx);
            }
            SessionMode::StoppingPlay(_) => {
                self.stopping_play(ctx);
            }
            SessionMode::PreparingToPlay(play_id) => {
                self.preparing_to_play(play_id, ctx);
            }
            SessionMode::PreparingToRender(render_id) => {
                self.preparing_to_render(render_id, ctx);
            }
            SessionMode::Rendering(render_id) => {
                self.rendering(render_id, ctx);
            }
            SessionMode::Playing(play_id) => {
                self.playing(play_id, ctx);
            }
            SessionMode::Idle => {
                self.idle(ctx);
            }
        }
    }

    fn stopping_render(&mut self, ctx: &mut Context<Self>) {
        if !self.engine_loaded {
            self.set_idle();
            return;
        }

        if !self.state.play_state.value().is_stopped() {
            if self.audio_engine_tracker.should_retry() {
                self.request_audio_engine_command(AudioEngineCommand::Stop {});
                self.audio_engine_tracker.retried();
            }
        } else {
            self.set_idle();
        }
    }

    fn set_idle(&mut self) {
        self.state.mode = SessionMode::Idle.into();
    }

    fn stopping_play(&mut self, ctx: &mut Context<Self>) {
        if !self.engine_loaded {
            self.set_idle();
            return;
        }

        if !self.state.play_state.value().is_stopped() {
            if self.audio_engine_tracker.should_retry() {
                self.request_audio_engine_command(AudioEngineCommand::Stop {});
                self.audio_engine_tracker.retried();
            }
        } else {
            self.set_idle();
        }
    }

    fn preparing_to_play(&mut self, play_id: &PlayId, ctx: &mut Context<Self>) {
        if !self.engine_loaded {
            return;
        }
    }

    fn preparing_to_render(&mut self, render_id: &RenderId, ctx: &mut Context<Self>) {
        if !self.engine_loaded {
            return;
        }
    }

    fn rendering(&mut self, render_id: &RenderId, ctx: &mut Context<Self>) {}

    fn playing(&mut self, play_id: &PlayId, ctx: &mut Context<Self>) {}

    fn idle(&mut self, ctx: &mut Context<Self>) {}

    fn emit_spec(&self) {
        self.issue_system_async(NotifySessionSpec { session_id: self.id.clone(),
                                                    spec:       self.session.spec.clone(), });
    }
    fn request_audio_engine_command(&self, cmd: AudioEngineCommand) {
        todo!()
    }
}

pub struct SessionsSupervisor {
    active:   HashMap<AppSessionId, Addr<SessionActor>>,
    sessions: HashMap<AppSessionId, Session>,
}

impl Actor for SessionsSupervisor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {}
}

impl Supervised for SessionsSupervisor {
    fn restarting(&mut self, ctx: &mut Self::Context) {
        self.subscribe_system_async::<NotifySessionSpec>(ctx);
        self.subscribe_system_async::<NotifySessionState>(ctx);
    }
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifySessionPacket {
    pub session_id: AppSessionId,
    pub packet:     SessionPacket,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifySessionSpec {
    pub session_id: AppSessionId,
    pub spec:       SessionSpec,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifySessionState {
    pub session_id: AppSessionId,
    pub state:      SessionState,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyAudioEngineEvent {
    pub session_id: AppSessionId,
    pub event:      AudioEngineEvent,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyRenderComplete {
    pub session_id: AppSessionId,
    pub render_id:  RenderId,
    pub path:       String,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyRenderFailed {
    pub session_id: AppSessionId,
    pub render_id:  RenderId,
    pub error:      String,
    pub cancelled:  bool,
}
