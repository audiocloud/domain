use actix_web::rt::Runtime;
use actix_web::{get, web, App, HttpServer, Responder};
use flume::Sender;
use serde_json::json;

use crate::audio_engine::ReaperEngineCommand;

pub fn run(tx_cmd: Sender<ReaperEngineCommand>) {
    Runtime::new().expect("Create runtime")
                  .block_on(http_server(tx_cmd))
                  .expect("Http server successfully started");
}

async fn http_server(tx_cmd: Sender<ReaperEngineCommand>) -> anyhow::Result<()> {
    let data = web::Data::new(tx_cmd);
    HttpServer::new(move || App::new().service(hello_world)).workers(1)
                                                            .bind(("127.0.0.1", 7300))?
                                                            .run()
                                                            .await?;

    Ok(())
}

#[get("/v1/status")]
async fn hello_world() -> impl Responder {
    web::Json(json!({"status": "ok"}))
}
