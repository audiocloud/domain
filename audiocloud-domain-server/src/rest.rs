use actix_web::{get, web, Responder};
use serde_json::json;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(healthz);
}

#[get("/healthz")]
async fn healthz() -> impl Responder {
    web::Json(json!({
                "healthy": true
              }))
}
