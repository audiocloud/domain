use actix_web::error::{ErrorInternalServerError, ErrorNotFound};
use actix_web::{delete, get, post, put, web, Error, Responder};
use anyhow::anyhow;
use serde_json::json;
use web::{Data, Json, Path};

use audiocloud_api::media::{DownloadFromDomain, ImportToDomain, MediaObject, UpdateMediaSession, UploadToDomain};
use audiocloud_api::newtypes::{AppId, AppMediaObjectId, AppSessionId, MediaObjectId, SessionId};

use crate::db::{Db, PersistedDownload, PersistedMediaObject, PersistedSession, PersistedUpload};

pub fn rest_api(cfg: &mut web::ServiceConfig) {
    cfg.service(get_media_state)
       .service(get_multiple_media_state)
       .service(create_upload)
       .service(create_download)
       .service(import_in_domain)
       .service(update_session)
       .service(delete_session);
}

#[get("apps/{app_id}/media/{media_object_id}")]
async fn get_media_state(db: Data<Db>, path: Path<(AppId, MediaObjectId)>) -> impl Responder {
    let (app_id, media_object_id) = path.into_inner();
    let id = AppMediaObjectId::new(app_id, media_object_id);

    let state = db.get_media_status(&id)
                  .await
                  .map_err(ErrorInternalServerError)?
                  .map(to_public_state);

    Ok::<_, Error>(web::Json(state))
}

#[post("/multiple")]
async fn get_multiple_media_state(db: Data<Db>, Json(ids): Json<Vec<AppMediaObjectId>>) -> impl Responder {
    let states = db.get_media_status_multiple(ids.iter())
                   .await
                   .map_err(ErrorInternalServerError)?
                   .into_iter()
                   .map(to_public_state)
                   .collect::<Vec<_>>();

    Ok::<_, Error>(web::Json(states))
}

#[post("apps/{app_id}/media/{media_object_id}/upload")]
async fn create_upload(db: Data<Db>,
                       path: Path<(AppId, MediaObjectId)>,
                       Json(upload): Json<UploadToDomain>)
                       -> impl Responder {
    let (app_id, media_object_id) = path.into_inner();
    let id = AppMediaObjectId::new(app_id, media_object_id);

    let state = db.update_media_status(&id, |media| {
                      media.metadata = Some(upload.metadata());
                      media.upload = PersistedUpload::new(Some(upload));
                  })
                  .await
                  .map_err(ErrorInternalServerError)?;

    Ok::<_, Error>(Json(to_public_state(state)))
}

#[post("apps/{app_id}/media/{media_object_id}/download")]
async fn create_download(db: Data<Db>,
                         path: Path<(AppId, MediaObjectId)>,
                         Json(download): Json<DownloadFromDomain>)
                         -> impl Responder {
    let (app_id, media_object_id) = path.into_inner();
    let id = AppMediaObjectId::new(app_id, media_object_id);

    let state = db.update_media_status(&id, |media| {
                      media.download = PersistedDownload::new(Some(download));
                  })
                  .await
                  .map_err(ErrorInternalServerError)?;

    Ok::<_, Error>(Json(to_public_state(state)))
}

#[post("apps/{app_id}/media/{media_object_id}/import")]
async fn import_in_domain(db: Data<Db>,
                          path: Path<(AppId, MediaObjectId)>,
                          Json(import): Json<ImportToDomain>)
                          -> impl Responder {
    let (app_id, media_object_id) = path.into_inner();
    let id = AppMediaObjectId::new(app_id, media_object_id);

    let state = db.update_media_status(&id, |media| {
                      media.metadata = Some(import.metadata());
                      media.path = Some(import.path);
                  })
                  .await
                  .map_err(ErrorInternalServerError)?;

    Ok::<_, Error>(Json(to_public_state(state)))
}

#[put("apps/{app_id}/sessions/{session_id}")]
async fn update_session(db: Data<Db>,
                        path: Path<(AppId, SessionId)>,
                        Json(data): Json<UpdateMediaSession>)
                        -> impl Responder {
    let (app_id, session_id) = path.into_inner();

    let persisted = PersistedSession { _id:           AppSessionId::new(app_id, session_id),
                                       ends_at:       data.ends_at,
                                       media_objects: data.media_objects, };

    let update_type = db.set_session_state(persisted)
                        .await
                        .map_err(ErrorInternalServerError)?;

    Ok::<_, Error>(Json(json!({ "update": update_type })))
}

#[delete("apps/{app_id}/sessions/{session_id}")]
async fn delete_session(db: Data<Db>, path: Path<(AppId, SessionId)>) -> impl Responder {
    let (app_id, session_id) = path.into_inner();
    let id = AppSessionId::new(app_id, session_id);

    let deleted = db.delete_session_state(&id).await.map_err(ErrorInternalServerError)?;
    if !deleted {
        return Err(ErrorNotFound(anyhow!("Session '{id}' not found")));
    }

    Ok::<_, Error>(web::Json(json!({ "deleted": deleted })))
}

fn to_public_state(state: PersistedMediaObject) -> MediaObject {
    let PersistedMediaObject { _id,
                               metadata,
                               path,
                               download,
                               upload, } = state;

    MediaObject { id: _id,
                  metadata,
                  path,
                  download: download.state,
                  upload: upload.state }
}
