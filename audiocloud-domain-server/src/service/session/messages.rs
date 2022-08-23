use std::collections::HashMap;

use actix::Message;

use audiocloud_api::app::SessionPacket;
use audiocloud_api::audio_engine::AudioEngineEvent;
use audiocloud_api::change::{DesiredSessionPlayState, RenderId, SessionState};
use audiocloud_api::cloud::apps::SessionSpec;
use audiocloud_api::domain::DomainSessionCommand;
use audiocloud_api::media::MediaServiceEvent;
use audiocloud_api::newtypes::{AppSessionId, AudioEngineId, MediaServiceId, SecureKey};
use audiocloud_api::session::SessionSecurity;

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct SetSessionDesiredState {
    pub session_id: AppSessionId,
    pub desired:    DesiredSessionPlayState,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "anyhow::Result<()>")]
pub struct ExecuteSessionCommand {
    pub session_id: AppSessionId,
    pub command:    DomainSessionCommand,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifySessionPacket {
    pub session_id: AppSessionId,
    pub packet:     SessionPacket,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifySessionDeleted {
    pub session_id: AppSessionId,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifySessionSecurity {
    pub session_id: AppSessionId,
    pub security:   HashMap<SecureKey, SessionSecurity>,
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
    pub engine_id: AudioEngineId,
    pub event:     AudioEngineEvent,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyMediaServiceEvent {
    pub media_service_id: MediaServiceId,
    pub event:            MediaServiceEvent,
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
