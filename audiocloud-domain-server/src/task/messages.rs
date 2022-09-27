use std::collections::HashMap;

use actix::Message;

use crate::DomainResult;
use audiocloud_api::audio_engine::event::AudioEngineEvent;
use audiocloud_api::common::change::{DesiredTaskPlayState, TaskState};
use audiocloud_api::common::media::{MediaObject, RenderId};
use audiocloud_api::common::task::TaskPermissions;
use audiocloud_api::common::task::TaskSpec;
use audiocloud_api::domain::tasks::TaskUpdated;
use audiocloud_api::domain::DomainCommand;
use audiocloud_api::newtypes::{AppMediaObjectId, AppTaskId, EngineId, SecureKey};
use audiocloud_api::{StreamingPacket, TaskReservation};

#[derive(Message, Clone, Debug)]
#[rtype(result = "DomainResult<TaskUpdated>")]
pub struct SetTaskDesiredState {
    pub task_id: AppTaskId,
    pub desired: DesiredTaskPlayState,
    pub version: u64,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "anyhow::Result<()>")]
pub struct ExecuteTaskCommand {
    pub session_id: AppTaskId,
    pub command:    DomainCommand,
    pub security:   TaskPermissions,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyStreamingPacket {
    pub session_id: AppTaskId,
    pub packet:     StreamingPacket,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyTaskDeleted {
    pub task_id: AppTaskId,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyTaskSecurity {
    pub session_id: AppTaskId,
    pub security:   HashMap<SecureKey, TaskPermissions>,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyTaskSpec {
    pub task_id: AppTaskId,
    pub spec:    TaskSpec,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyTaskReservation {
    pub task_id: AppTaskId,
    pub reservation:    TaskReservation,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyTaskState {
    pub session_id: AppTaskId,
    pub state:      TaskState,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyAudioEngineEvent {
    pub engine_id: EngineId,
    pub event:     AudioEngineEvent,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyMediaTaskState {
    pub task_id: AppTaskId,
    pub media:   HashMap<AppMediaObjectId, MediaObject>,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyRenderComplete {
    pub task_id:    AppTaskId,
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
    pub task_id:   AppTaskId,
    pub render_id: RenderId,
    pub error:     String,
    pub cancelled: bool,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct BecomeOnline;
