use actix::SystemService;
use actix_web::error::ErrorInternalServerError;
use actix_web::{delete, get, post, put, web, Error, Responder};
use serde_json::json;

use audiocloud_api::change::ModifySessionSpec;
use audiocloud_api::cloud::apps::CreateSession;
use audiocloud_api::domain::DomainSessionCommand;
use audiocloud_api::newtypes::{AppId, AppSessionId, SessionId};

use crate::service::session::messages::ExecuteSessionCommand;
use crate::service::session::supervisor::SessionsSupervisor;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(healthz);
    cfg.service(web::scope("/v1").configure(rest_api));
}

#[get("/healthz")]
async fn healthz() -> impl Responder {
    let res = json!({
      "healthy": true
    });

    web::Json(res)
}

fn rest_api(cfg: &mut web::ServiceConfig) {
    cfg.service(create_session)
       .service(modify_session)
       .service(delete_session);
}

#[post("/apps/{app_id}/sessions/{session_id}")]
async fn create_session(path: web::Path<(AppId, SessionId)>, create: web::Json<CreateSession>) -> impl Responder {
    let (app_id, id) = path.into_inner();
    let id = AppSessionId::new(app_id, id);

    Ok::<_, Error>(web::Json(SessionsSupervisor::from_registry().send(ExecuteSessionCommand {
        session_id: id.clone(),
        command: DomainSessionCommand::Create {app_session_id: id.clone(), create: create.into_inner()}
    }).await.map_err(ErrorInternalServerError)?.map_err(ErrorInternalServerError)?))
}

#[post("/apps/{app_id}/sessions/{session_id}/modify-spec")]
async fn modify_session(path: web::Path<(AppId, SessionId)>,
                        modify: web::Json<Vec<ModifySessionSpec>>)
                        -> impl Responder {
    let (app_id, id) = path.into_inner();
    let id = AppSessionId::new(app_id, id);

    Ok::<_, Error>(web::Json(SessionsSupervisor::from_registry().send(ExecuteSessionCommand {
        session_id: id.clone(),
        command: DomainSessionCommand::Modify {app_session_id: id.clone(), modifications: modify.into_inner(), version: 0}
    }).await.map_err(ErrorInternalServerError)?.map_err(ErrorInternalServerError)?))
}

#[delete("/apps/{app_id}/sessions/{session_id}")]
async fn delete_session(path: web::Path<(AppId, SessionId)>) -> impl Responder {
    let (app_id, id) = path.into_inner();
    let id = AppSessionId::new(app_id, id);

    Ok::<_, Error>(web::Json(SessionsSupervisor::from_registry().send(ExecuteSessionCommand {
        session_id: id.clone(),
        command: DomainSessionCommand::Delete {app_session_id: id.clone()}
    }).await.map_err(ErrorInternalServerError)?.map_err(ErrorInternalServerError)?))
}
