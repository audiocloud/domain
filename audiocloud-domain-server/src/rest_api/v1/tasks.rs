use actix_web::web::Json;
use actix_web::{delete, get, post, web};
use serde::Deserialize;
use web::Path;

use audiocloud_api::audio_engine::{TaskPlayStopped, TaskPlaying, TaskRenderCancelled, TaskRendering, TaskSought};
use audiocloud_api::domain::tasks::{
    CreateTask, ModifyTask, TaskCreated, TaskDeleted, TaskSummaryList, TaskUpdated, TaskWithStatusAndSpec,
};
use audiocloud_api::domain::DomainError;
use audiocloud_api::{AppId, RequestCancelRender, RequestPlay, RequestRender, RequestSeek, RequestStopPlay, TaskId};

use crate::rest_api::{ApiResponder, ApiResponse};

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

fn not_implemented_yet<T>(call: &'static str) -> Result<T, DomainError> {
    Err(DomainError::NotImplemented { call:   call.to_string(),
                                      reason: "Not implemented yet".to_string(), })
}

#[get("/")]
async fn list_tasks(responder: ApiResponder) -> ApiResponse<TaskSummaryList> {
    responder.respond(async move { not_implemented_yet("list_tasks") })
             .await
}

#[post("/")]
async fn create_task(responder: ApiResponder, create: Json<CreateTask>) -> ApiResponse<TaskCreated> {
    responder.respond(async move { not_implemented_yet("create_task") })
             .await
}

#[get("/{app_id}/{task_id}")]
async fn get_task(responder: ApiResponder, task_id: Path<AppTaskIdPath>) -> ApiResponse<TaskWithStatusAndSpec> {
    responder.respond(async move { not_implemented_yet("get_task") }).await
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
