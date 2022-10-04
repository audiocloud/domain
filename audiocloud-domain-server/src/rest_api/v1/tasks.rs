use std::convert::identity;

use actix::MailboxError;
use actix_web::web::Json;
use actix_web::{delete, get, post, web};
use futures::FutureExt;
use serde::Deserialize;
use web::Path;

use audiocloud_api::audio_engine::{TaskPlayStopped, TaskPlaying, TaskRenderCancelled, TaskRendering, TaskSought};
use audiocloud_api::domain::tasks::{
    CreateTask, ModifyTask, TaskCreated, TaskDeleted, TaskSummaryList, TaskUpdated, TaskWithStatusAndSpec,
};
use audiocloud_api::domain::DomainError;
use audiocloud_api::{
    AppId, AppTaskId, RequestCancelRender, RequestPlay, RequestRender, RequestSeek, RequestStopPlay, TaskId,
};

use crate::rest_api::{ApiResponder, ApiResponse};
use crate::tasks::{get_tasks_supervisor, messages, ListTasks};

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_tasks)
       .service(create_task)
       .service(get_task)
       .service(modify_task)
       .service(delete_task)
       .service(render_task)
       .service(play_task)
       .service(seek_task)
       .service(cancel_render_task)
       .service(stop_play_task);
}

#[derive(Deserialize)]
struct AppTaskIdPath {
    app_id:  AppId,
    task_id: TaskId,
}

impl Into<AppTaskId> for AppTaskIdPath {
    fn into(self) -> AppTaskId {
        let Self { app_id, task_id } = self;
        AppTaskId { app_id, task_id }
    }
}

fn bad_gateway(err: MailboxError) -> DomainError {
    DomainError::BadGateway { error: err.to_string() }
}

fn not_implemented_yet<T>(call: &'static str) -> Result<T, DomainError> {
    Err(DomainError::NotImplemented { call:   call.to_string(),
                                      reason: "Not implemented yet".to_string(), })
}

#[get("/")]
async fn list_tasks(responder: ApiResponder) -> ApiResponse<TaskSummaryList> {
    responder.respond(async move { get_tasks_supervisor().send(ListTasks).await.map_err(bad_gateway) })
             .await
}

#[post("/")]
async fn create_task(responder: ApiResponder, create: Json<CreateTask>) -> ApiResponse<TaskCreated> {
    let create = messages::CreateTask { app_session_id: create.0.task_id,
                                        reservations:   create.0.reservations,
                                        spec:           create.0.spec,
                                        security:       create.0.security, };

    responder.respond(async move {
                 get_tasks_supervisor().send(create)
                                       .await
                                       .map_err(bad_gateway)
                                       .and_then(identity)
             })
             .await
}

#[get("/{app_id}/{task_id}")]
async fn get_task(responder: ApiResponder, task_id: Path<AppTaskIdPath>) -> ApiResponse<TaskWithStatusAndSpec> {
    let task_id = task_id.into_inner().into();
    let get = messages::GetTaskWithStatusAndSpec { task_id };
    responder.respond(async move {
                 get_tasks_supervisor().send(get)
                                       .await
                                       .map_err(bad_gateway)
                                       .and_then(identity)
             })
             .await
}

#[post("/{app_id}/{task_id}/modify")]
async fn modify_task(responder: ApiResponder,
                     task_id: Path<AppTaskIdPath>,
                     modify: Json<ModifyTask>)
                     -> ApiResponse<TaskUpdated> {
    responder.respond(async move { not_implemented_yet("modify_task") })
             .await
}

#[delete("/{app_id}/{task_id}")]
async fn delete_task(responder: ApiResponder, task_id: Path<AppTaskIdPath>) -> ApiResponse<TaskDeleted> {
    responder.respond(async move { not_implemented_yet("delete_task") })
             .await
}

#[post("/{app_id}/{task_id}/transport/render")]
async fn render_task(responder: ApiResponder,
                     task_id: Path<AppTaskIdPath>,
                     render: Json<RequestRender>)
                     -> ApiResponse<TaskRendering> {
    responder.respond(async move { not_implemented_yet("render_task") })
             .await
}

#[post("/{app_id}/{task_id}/transport/play")]
async fn play_task(responder: ApiResponder,
                   task_id: Path<AppTaskIdPath>,
                   play: Json<RequestPlay>)
                   -> ApiResponse<TaskPlaying> {
    responder.respond(async move { not_implemented_yet("play_task") }).await
}

#[post("/{app_id}/{task_id}/transport/seek")]
async fn seek_task(responder: ApiResponder,
                   task_id: Path<AppTaskIdPath>,
                   seek: Json<RequestSeek>)
                   -> ApiResponse<TaskSought> {
    responder.respond(async move { not_implemented_yet("seek_task") }).await
}

#[post("/{app_id}/{task_id}/transport/cancel")]
async fn cancel_render_task(responder: ApiResponder,
                            task_id: Path<AppTaskIdPath>,
                            cancel: Json<RequestCancelRender>)
                            -> ApiResponse<TaskRenderCancelled> {
    responder.respond(async move { not_implemented_yet("cancel_render_task") })
             .await
}

#[post("/{app_id}/{task_id}/transport/stop")]
async fn stop_play_task(responder: ApiResponder,
                        task_id: Path<AppTaskIdPath>,
                        stop: Json<RequestStopPlay>)
                        -> ApiResponse<TaskPlayStopped> {
    responder.respond(async move { not_implemented_yet("stop_play_task") })
             .await
}
