use std::collections::HashMap;
use std::mem;
use std::time::Duration;

use actix::{Actor, ArbiterHandle, AsyncContext, Context, Handler, Message, Running, Supervised};
use actix_broker::{BrokerIssue, BrokerSubscribe};

use audiocloud_api::app::SessionPacket;
use audiocloud_api::audio_engine::{AudioEngineCommand, AudioEngineEvent};
use audiocloud_api::change::{DesiredSessionPlayState, SessionState};
use audiocloud_api::cloud::apps::SessionSpec;
use audiocloud_api::newtypes::{AppSessionId, FixedInstanceId};
use audiocloud_api::time::Timestamped;

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
        self.subscribe_system_async::<NotifySessionPacket>(ctx);

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
            AudioEngineEvent::Stopped => {}
            AudioEngineEvent::Playing { .. } => {}
            AudioEngineEvent::Rendering { .. } => {}
            AudioEngineEvent::RenderingFinished { .. } => {}
            AudioEngineEvent::Meters { .. } => {}
            AudioEngineEvent::Exit { .. } => {
                self.engine_loaded = false;
            }
        }
    }
}

impl SessionActor {
    fn update(&mut self, ctx: &mut Context<Self>) {
        if !self.state
                .play_state
                .value()
                .satisfies(self.state.desired_play_state.value())
        {
            if self.audio_engine_tracker.should_retry() {
                // TODO: request the appropriate command for the audio engine
                // self.request_audio_engine_command();
                self.audio_engine_tracker.retried();
            }
            return;
        }
    }

    fn emit_spec(&self) {
        self.issue_system_async(NotifySessionSpec { session_id: self.id.clone(),
                                                    spec:       self.session.spec.clone(), });
    }
    fn request_audio_engine_command(&self, cmd: AudioEngineCommand) {
        todo!()
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
pub struct NotifyAudioEngineEvent {
    pub session_id: AppSessionId,
    pub event:      AudioEngineEvent,
}
