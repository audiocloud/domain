use actix_web::error::ErrorInternalServerError;
use actix_web::{get, post, web, Error, Responder};
use web::{Data, Json, Path};

use audiocloud_api::media::{DownloadFromDomain, UploadToDomain};
use audiocloud_api::newtypes::{AppId, AppMediaObjectId, MediaObjectId};

use crate::db::Db;

pub fn rest_api(cfg: &mut web::ServiceConfig) {
    cfg.service(get_media_state)
       .service(create_upload)
       .service(create_download);
}

#[get("apps/{app_id}/media/{media_object_id}")]
async fn get_media_state(db: Data<Db>, path: Path<(AppId, MediaObjectId)>) -> impl Responder {
    let (app_id, media_object_id) = path.into_inner();
    let id = AppMediaObjectId::new(app_id, media_object_id);

    let state = db.get_media_status(&id).await.map_err(ErrorInternalServerError)?;

    Ok::<_, Error>(web::Json(state))
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
                      media.upload = Some(upload);
                  })
                  .await
                  .map_err(ErrorInternalServerError)?;

    Ok::<_, Error>(Json(state))
}

#[post("apps/{app_id}/media/{media_object_id}/download")]
async fn create_download(db: Data<Db>,
                         path: Path<(AppId, MediaObjectId)>,
                         Json(download): Json<DownloadFromDomain>)
                         -> impl Responder {
    let (app_id, media_object_id) = path.into_inner();
    let id = AppMediaObjectId::new(app_id, media_object_id);

    let state = db.update_media_status(&id, |media| {
                      media.download = Some(download);
                  })
                  .await
                  .map_err(ErrorInternalServerError)?;

    Ok::<_, Error>(Json(state))
}
