use std::collections::HashMap;
use std::time::Duration;

use actix_web::error::ErrorInternalServerError;
use actix_web::rt::time::timeout;
use actix_web::rt::Runtime;
use actix_web::{get, put, web, App, Error, HttpServer, Responder};
use flume::Sender;
use serde::{Deserialize, Serialize};

use audiocloud_api::audio_engine::AudioEngineCommand;
use audiocloud_api::cloud::apps::SessionSpec;
use audiocloud_api::cloud::domains::InstanceRouting;
use audiocloud_api::newtypes::{AppId, AppMediaObjectId, AppSessionId, FixedInstanceId, SessionId};

use crate::audio_engine::{EngineStatus, ReaperEngineCommand};

pub fn run(tx_cmd: Sender<ReaperEngineCommand>) {
    Runtime::new().expect("Create runtime")
                  .block_on(http_server(tx_cmd))
                  .expect("Http server successfully started");
}

async fn http_server(tx_cmd: Sender<ReaperEngineCommand>) -> anyhow::Result<()> {
    let data = web::Data::new(AudioEngineClient(tx_cmd));
    HttpServer::new(move || {
        App::new().app_data(data.clone())
                  .service(get_status)
                  .service(create_session)
    }).workers(1)
      .bind(("127.0.0.1", 7300))?
      .run()
      .await?;

    Ok(())
}

#[derive(Clone)]
struct AudioEngineClient(Sender<ReaperEngineCommand>);

impl AudioEngineClient {
    async fn request<R>(&self, f: impl FnOnce(Sender<anyhow::Result<R>>) -> ReaperEngineCommand) -> anyhow::Result<R> {
        let (tx, rx) = flume::unbounded();
        self.0.send_async(f(tx)).await?;
        Ok(timeout(Duration::from_secs(2), rx.recv_async()).await???)
    }

    pub async fn get_status(&self) -> anyhow::Result<HashMap<AppSessionId, EngineStatus>> {
        self.request(move |tx| ReaperEngineCommand::GetStatus(tx)).await
    }

    pub async fn set_session_spec(&self,
                                  session_id: AppSessionId,
                                  spec: SessionSpec,
                                  instances: HashMap<FixedInstanceId, InstanceRouting>,
                                  media_ready: HashMap<AppMediaObjectId, String>)
                                  -> anyhow::Result<()> {
        self.request(move |tx| {
                ReaperEngineCommand::Request((AudioEngineCommand::SetSpec { session_id,
                                                                            spec,
                                                                            instances,
                                                                            media_ready },
                                              tx))
            })
            .await
    }
}

#[get("/v1/status")]
async fn get_status(client: web::Data<AudioEngineClient>) -> impl Responder {
    Ok::<_, Error>(web::Json(client.get_status().await.map_err(ErrorInternalServerError)?))
}

#[put("/v1/apps/{app_id}/sessions/{session_id}/spec")]
async fn create_session(client: web::Data<AudioEngineClient>,
                        path: web::Path<(AppId, SessionId)>,
                        body: web::Json<SetSessionSpec>)
                        -> impl Responder {
    let (app_id, session_id) = path.into_inner();
    let id = AppSessionId::new(app_id, session_id);
    let body = body.into_inner();

    Ok::<_, Error>(web::Json(client.set_session_spec(id, body.session, body.instances, body.media_ready)
                                   .await
                                   .map_err(ErrorInternalServerError)?))
}

#[derive(Deserialize, Serialize)]
struct SetSessionSpec {
    session:     SessionSpec,
    instances:   HashMap<FixedInstanceId, InstanceRouting>,
    media_ready: HashMap<AppMediaObjectId, String>,
}
