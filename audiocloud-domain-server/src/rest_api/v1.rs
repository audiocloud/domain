use actix::SystemService;
use actix_web::error::ErrorInternalServerError;
use actix_web::{delete, post, web, Error, Responder};

use audiocloud_api::common::change::ModifyTaskSpec;
use audiocloud_api::cloud::tasks::CreateTask;
use audiocloud_api::domain::DomainSessionCommand;
use audiocloud_api::newtypes::{AppId, AppTaskId, TaskId};
use audiocloud_api::common::task::TaskPermissions;

use crate::service::session::messages::ExecuteSessionCommand;
use crate::service::session::supervisor::SessionsSupervisor;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(create_session)
       .service(modify_session)
       .service(delete_session);
}

#[post("/apps/{app_id}/sessions/{session_id}")]
async fn create_session(path: web::Path<(AppId, TaskId)>, create: web::Json<CreateTask>) -> impl Responder {
    let (app_id, id) = path.into_inner();
    let id = AppTaskId::new(app_id, id);

    Ok::<_, Error>(web::Json(SessionsSupervisor::from_registry().send(ExecuteSessionCommand {
        session_id: id.clone(),
        command: DomainSessionCommand::Create {app_session_id: id.clone(), create: create.into_inner()},
        security: TaskPermissions::full()
    }).await.map_err(ErrorInternalServerError)?.map_err(ErrorInternalServerError)?))
}

#[post("/apps/{app_id}/sessions/{session_id}/modify-spec")]
async fn modify_session(path: web::Path<(AppId, TaskId)>,
                        modify: web::Json<Vec<ModifyTaskSpec>>)
                        -> impl Responder {
    let (app_id, id) = path.into_inner();
    let id = AppTaskId::new(app_id, id);

    Ok::<_, Error>(web::Json(SessionsSupervisor::from_registry().send(ExecuteSessionCommand {
        session_id: id.clone(),
        command: DomainSessionCommand::Modify {app_session_id: id.clone(), modifications: modify.into_inner(), version: 0},
        security: TaskPermissions::full()
    }).await.map_err(ErrorInternalServerError)?.map_err(ErrorInternalServerError)?))
}

#[delete("/apps/{app_id}/sessions/{session_id}")]
async fn delete_session(path: web::Path<(AppId, TaskId)>) -> impl Responder {
    let (app_id, id) = path.into_inner();
    let id = AppTaskId::new(app_id, id);

    Ok::<_, Error>(web::Json(SessionsSupervisor::from_registry().send(ExecuteSessionCommand {
        session_id: id.clone(),
        command: DomainSessionCommand::Delete {app_session_id: id.clone()},
        security: TaskPermissions::full()
    }).await.map_err(ErrorInternalServerError)?.map_err(ErrorInternalServerError)?))
}
