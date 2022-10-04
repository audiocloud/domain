use std::collections::HashMap;

use actix::Message;

use audiocloud_api::audio_engine::event::EngineEvent;
use audiocloud_api::common::change::{DesiredTaskPlayState, TaskState};
use audiocloud_api::common::media::{MediaObject, RenderId};
use audiocloud_api::common::task::TaskPermissions;
use audiocloud_api::common::task::TaskSpec;
use audiocloud_api::domain::tasks::{TaskCreated, TaskDeleted, TaskSummaryList, TaskUpdated, TaskWithStatusAndSpec};
use audiocloud_api::newtypes::{AppMediaObjectId, AppTaskId, EngineId, SecureKey};
use audiocloud_api::{CreateTaskReservation, CreateTaskSecurity, CreateTaskSpec, StreamingPacket, TaskReservation};

use crate::DomainResult;

#[derive(Message, Clone, Debug)]
#[rtype(result = "DomainResult<TaskUpdated>")]
pub struct SetTaskDesiredPlayState {
    pub task_id: AppTaskId,
    pub desired: DesiredTaskPlayState,
    pub version: u64,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "DomainResult<TaskCreated>")]
pub struct CreateTask {
    pub task_id: AppTaskId,
    pub reservations:   CreateTaskReservation,
    pub spec:           CreateTaskSpec,
    pub security:       CreateTaskSecurity,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "DomainResult<TaskDeleted>")]
pub struct DeleteTask {
    pub app_session_id: AppTaskId,
    pub version:        u64,
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
pub struct NotifyTaskActivated {
    pub task_id: AppTaskId,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyTaskDeactivated {
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
    pub task_id:     AppTaskId,
    pub reservation: TaskReservation,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyTaskState {
    pub task_id: AppTaskId,
    pub state:   TaskState,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyEngineEvent {
    pub engine_id: EngineId,
    pub event:     EngineEvent,
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

#[derive(Message, Clone, Debug)]
#[rtype(result = "TaskSummaryList")]
pub struct ListTasks;

#[derive(Message, Clone, Debug)]
#[rtype(result = "DomainResult<TaskWithStatusAndSpec>")]
pub struct GetTaskWithStatusAndSpec {
    pub task_id: AppTaskId,
}
