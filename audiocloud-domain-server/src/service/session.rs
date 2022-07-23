use std::collections::HashMap;
use std::time::Duration;

use actix::{Actor, Addr, AsyncContext, Context, Handler, Message, Supervised, SystemService};
use actix_broker::{BrokerIssue, BrokerSubscribe};

use crate::data::get_boot_cfg;
use audiocloud_api::app::SessionPacket;
use audiocloud_api::audio_engine::{AudioEngineCommand, AudioEngineEvent};
use audiocloud_api::change::{
    DesiredSessionPlayState, PlayId, PlaySession, RenderId, RenderSession, SessionPlayState, SessionState,
};
use audiocloud_api::cloud::apps::SessionSpec;
use audiocloud_api::instance::DesiredInstancePlayState;
use audiocloud_api::newtypes::AppSessionId;
use audiocloud_api::session::{Session, SessionMode};

use crate::service::instance::{NotifyInstanceError, NotifyInstanceReports, NotifyInstanceState};
use crate::tracker::RequestTracker;

pub mod session_instances;

pub struct SessionActor {
    id:                   AppSessionId,
    session:              Session,
    packet:               SessionPacket,
    instances:            session_instances::SessionInstances,
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
                self.packet.push_audio_packets(audio);
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
                // TODO: translate to packet
            }
            AudioEngineEvent::Exit { .. } => {
                self.engine_loaded = false;
            }
        }
    }
}

impl Handler<SetSessionDesiredState> for SessionActor {
    type Result = ();

    fn handle(&mut self, msg: SetSessionDesiredState, ctx: &mut Self::Context) -> Self::Result {
        if self.state.desired_play_state.value() != &msg.desired {
            self.state.desired_play_state = msg.desired.into();
            self.audio_engine_tracker.reset();

            self.stop();
            self.update(ctx);
        }
    }
}

impl SessionActor {
    fn update(&mut self, ctx: &mut Context<Self>) {
        let mut modified = false;
        match (self.state.mode.value(), self.state.desired_play_state.value()) {
            (SessionMode::Idle, DesiredSessionPlayState::Play(play)) => {
                modified = true;
                self.prepare_to_play(play.play_id.clone());
            }
            (SessionMode::Idle, DesiredSessionPlayState::Render(render)) => {
                modified = true;
                self.prepare_to_render(render.render_id.clone(), render.segment.length);
            }
            (SessionMode::Rendering(render_id), desired) => {
                if !matches!(desired, DesiredSessionPlayState::Render(r) if render_id == &r.render_id) {
                    modified = true;
                    self.prepare_render_stop(render_id.clone());
                }
            }
            (SessionMode::Playing(play_id), desired) => {
                if !matches!(desired, DesiredSessionPlayState::Play(p) if play_id == &p.play_id) {
                    modified = true;
                    self.prepare_play_stop(play_id.clone());
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
            SessionMode::Rendering(_) | SessionMode::Playing(_) => {}
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

    fn update_stopping(&mut self, ctx: &mut Context<Self>) {
        if !self.state.play_state.value().is_stopped() {
            if self.audio_engine_tracker.should_retry() {
                self.request_audio_engine_command(AudioEngineCommand::Stop {});
                self.audio_engine_tracker.retried();
            }
        } else if self.engine_loaded {
            self.set_idle();
        }
    }

    fn set_idle(&mut self) {
        self.state.mode = SessionMode::Idle.into();
    }

    fn update_play_state(&mut self, ctx: &mut Context<Self>) {
        if !self.instances.update(&self.session.spec) || !self.engine_loaded {
            return;
        }

        // instances are fine, audio engine is also loaded
        if !self.state
                .play_state
                .value()
                .satisfies(self.state.desired_play_state.value())
        {
            if self.audio_engine_tracker.should_retry() {
                let command = match self.state.desired_play_state.value() {
                    DesiredSessionPlayState::Play(play) => AudioEngineCommand::Play(play.clone()),
                    DesiredSessionPlayState::Render(render) => AudioEngineCommand::Render(render.clone()),
                    DesiredSessionPlayState::Stopped => AudioEngineCommand::Stop,
                };
                self.request_audio_engine_command(command);
                self.audio_engine_tracker.retried();
            }
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

    fn preparing_to_render(&mut self, ctx: &mut Context<Self>) {
        if !self.engine_loaded {
            return;
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

    fn request_audio_engine_command(&self, cmd: AudioEngineCommand) {
        todo!()
    }
}

pub struct SessionsSupervisor {
    active:   HashMap<AppSessionId, Addr<SessionActor>>,
    sessions: HashMap<AppSessionId, Session>,
    state:    HashMap<AppSessionId, SessionState>,
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

impl Default for SessionsSupervisor {
    fn default() -> Self {
        let sessions = get_boot_cfg().sessions.clone();
        Self { active: Default::default(),
               sessions,
               state: Default::default() }
    }
}

impl SystemService for SessionsSupervisor {}

impl Handler<NotifySessionSpec> for SessionsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifySessionSpec, ctx: &mut Self::Context) -> Self::Result {
        if let Some(session) = self.sessions.get_mut(&msg.session_id) {
            session.spec = msg.spec;
        }
    }
}

impl Handler<SetSessionDesiredState> for SessionsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: SetSessionDesiredState, ctx: &mut Self::Context) -> Self::Result {
        if let Some(session) = self.active.get_mut(&msg.session_id) {
            session.do_send(msg);
        }
    }
}

impl Handler<NotifySessionState> for SessionsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifySessionState, ctx: &mut Self::Context) -> Self::Result {
        self.state.insert(msg.session_id, msg.state);
    }
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct SetSessionDesiredState {
    pub session_id: AppSessionId,
    pub desired:    DesiredSessionPlayState,
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

pub fn init() {
    let _ = SessionsSupervisor::from_registry();
}
