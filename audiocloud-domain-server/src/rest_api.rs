use actix_web::{get, web, Responder};
use serde_json::json;

mod v1;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(healthz);
    cfg.service(web::scope("/v1").configure(v1::configure));
}

#[get("/healthz")]
async fn healthz() -> impl Responder {
    let res = json!({
      "healthy": true
    });

    web::Json(res)
}
