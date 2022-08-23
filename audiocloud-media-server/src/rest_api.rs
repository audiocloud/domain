use actix_web::error::ErrorInternalServerError;
use actix_web::{get, post, web, Error, Responder};
use web::{Data, Json, Path};

use audiocloud_api::media::{DownloadFromDomain, ImportToDomain, MediaObject, UploadToDomain};
use audiocloud_api::newtypes::{AppId, AppMediaObjectId, MediaObjectId};

use crate::db::{Db, PersistedDownload, PersistedMediaObject, PersistedUpload};

pub fn rest_api(cfg: &mut web::ServiceConfig) {
    cfg.service(get_media_state)
       .service(get_multiple_media_state)
       .service(create_upload)
       .service(create_download)
       .service(import_in_domain);
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
                      media.upload = Some(PersistedUpload::new(upload));
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
                      media.download = Some(PersistedDownload::new(download));
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

fn to_public_state(state: PersistedMediaObject) -> MediaObject {
    let PersistedMediaObject { _id,
                               metadata,
                               path,
                               download,
                               upload, } = state;

    MediaObject { id: _id,
                  metadata,
                  path,
                  download: download.map(|d| d.state),
                  upload: upload.map(|u| u.state) }
}
