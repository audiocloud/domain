use std::collections::HashMap;

use actix::Message;

use audiocloud_api::domain::streaming::TaskStreamingPacket;
use audiocloud_api::audio_engine::event::AudioEngineEvent;
use audiocloud_api::common::change::{DesiredTaskPlayState, SessionState};
use audiocloud_api::common::task::TaskSpec;
use audiocloud_api::domain::DomainSessionCommand;
use audiocloud_api::common::media::{MediaObject, RenderId};
use audiocloud_api::newtypes::{AppMediaObjectId, AppTaskId, EngineId, SecureKey};
use audiocloud_api::common::task::TaskPermissions;

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct SetSessionDesiredState {
    pub session_id: AppTaskId,
    pub desired: DesiredTaskPlayState,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "anyhow::Result<()>")]
pub struct ExecuteSessionCommand {
    pub session_id: AppTaskId,
    pub command:    DomainSessionCommand,
    pub security: TaskPermissions,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifySessionPacket {
    pub session_id: AppTaskId,
    pub packet: TaskStreamingPacket,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifySessionDeleted {
    pub session_id: AppTaskId,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifySessionSecurity {
    pub session_id: AppTaskId,
    pub security:   HashMap<SecureKey, TaskPermissions>,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifySessionSpec {
    pub session_id: AppTaskId,
    pub spec: TaskSpec,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifySessionState {
    pub session_id: AppTaskId,
    pub state:      SessionState,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyAudioEngineEvent {
    pub engine_id: EngineId,
    pub event:     AudioEngineEvent,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyMediaSessionState {
    pub session_id: AppTaskId,
    pub media:      HashMap<AppMediaObjectId, MediaObject>,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyRenderComplete {
    pub session_id: AppTaskId,
    pub render_id:  RenderId,
    pub path:       String,
    pub object_id:  AppMediaObjectId,
    pub put_url:    String,
    pub notify_url: String,
    pub context:    String,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyRenderFailed {
    pub session_id: AppTaskId,
    pub render_id:  RenderId,
    pub error:      String,
    pub cancelled:  bool,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct BecomeOnline;
