use actix_web::{get, web};
use serde::Deserialize;

use audiocloud_api::domain::streaming::StreamStats;
use audiocloud_api::domain::DomainError;
use audiocloud_api::{AppId, PlayId, StreamingPacket, TaskId};

use crate::rest_api::{ApiResponder, ApiResponse};

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(get_stream_stats).service(get_stream_packet);
}

#[derive(Deserialize)]
struct AppTaskPlayId {
    app_id:  AppId,
    task_id: TaskId,
    play_id: PlayId,
}

#[derive(Deserialize)]
struct AppTaskPlayIdPacket {
    app_id:  AppId,
    task_id: TaskId,
    play_id: PlayId,
    serial:  u64,
}

#[get("/{app_id}/{task_id}/{play_id}")]
pub async fn get_stream_stats(path: web::Path<AppTaskPlayId>, responder: ApiResponder) -> ApiResponse<StreamStats> {
    responder.respond(async move {
                 Err(DomainError::NotImplemented { call:   "get_stream_stats".to_owned(),
                                                   reason: format!("not implemented yet"), })
             })
             .await
}

#[get("/{app_id}/{task_id}/{play_id}/packet/{serial}")]
pub async fn get_stream_packet(path: web::Path<AppTaskPlayIdPacket>,
                               responder: ApiResponder)
                               -> ApiResponse<StreamingPacket> {
    responder.respond(async move {
                 Err(DomainError::NotImplemented { call:   "get_stream_packet".to_owned(),
                                                   reason: format!("not implemented yet"), })
             })
             .await
}
