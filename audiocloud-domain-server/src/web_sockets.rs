#![allow(unused_variables)]

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::SeqCst;
use std::time::{Duration, Instant};

use actix::{
    fut, Actor, ActorContext, ActorFutureExt, AsyncContext, ContextFutureSpawner, Handler, MailboxError, StreamHandler,
    SystemService, WrapFuture,
};
use actix_web::{get, web, HttpRequest, Responder};
use actix_web_actors::ws;
use actix_web_actors::ws::WebsocketContext;
use maplit::hashmap;
use serde::Deserialize;
use tracing::*;

use audiocloud_api::common::task::TaskPermissions;
use audiocloud_api::domain::streaming::{DomainClientMessage, DomainServerMessage};
use audiocloud_api::domain::tasks::TaskUpdated;
use audiocloud_api::domain::DomainError;
use audiocloud_api::newtypes::{AppTaskId, SecureKey};
use audiocloud_api::{Codec, Json, MsgPack, RequestId, SerializableResult};
use messages::{RegisterWebSocket, WebSocketSend};
use supervisor::SocketsSupervisor;

use crate::task::messages::{NotifyTaskSecurity, SetTaskDesiredState};
use crate::task::supervisor::SessionsSupervisor;
use crate::{DomainResult, ResponseMedia};

mod messages;
mod supervisor;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(ws_handler);
}

#[derive(Deserialize)]
struct AuthParams {
    secure_key: SecureKey,
}

#[get("/ws")]
async fn ws_handler(req: HttpRequest, stream: web::Payload) -> impl Responder {
    let resp = ws::start(WebSocketActor { id:                  get_next_socket_id(),
                                          security:            hashmap! {},
                                          secure_key:          hashmap! {},
                                          security_updated_at: Instant::now(), },
                         &req,
                         stream);
    resp
}

#[derive(Debug)]
pub struct WebSocketActor {
    id:                  WebSocketId,
    security:            HashMap<AppTaskId, TaskPermissions>,
    secure_key:          HashMap<AppTaskId, SecureKey>,
    security_updated_at: Instant,
}

impl WebSocketActor {
    fn update(&mut self, ctx: &mut <Self as Actor>::Context) {
        if self.security.is_empty() && self.security_updated_at.elapsed().as_secs() > 10 {
            ctx.stop();
        }
    }

    fn submit(&mut self, msg: DomainClientMessage, media: ResponseMedia, ctx: &mut <Self as Actor>::Context) {
        match msg {
            DomainClientMessage::RequestSetDesiredPlayState { request_id,
                                                              task_id,
                                                              desired,
                                                              version, } => {
                let msg = SetTaskDesiredState { task_id,
                                                   desired,
                                                   version };

                ctx.spawn(SessionsSupervisor::from_registry().send(msg)
                                                             .into_actor(self)
                                                             .map(move |res, _, ctx| {
                                                                 Self::respond_set_desired_state(res, media,
                                                                                                 request_id, ctx)
                                                             }));
            }
            DomainClientMessage::RequestPeerConnection { .. } => {}
            DomainClientMessage::SubmitPeerConnectionCandidate { .. } => {}
            DomainClientMessage::RequestAttachToTask { .. } => {}
            DomainClientMessage::RequestDetachFromTask { .. } => {}
            DomainClientMessage::RequestModifyTaskSpec { .. } => {}
        }
    }

    fn respond_set_desired_state(result: Result<DomainResult<TaskUpdated>, MailboxError>,
                                 media: ResponseMedia,
                                 request_id: RequestId,
                                 ctx: &mut WebsocketContext<WebSocketActor>) {
        let result = match result {
            Ok(res) => match res {
                Ok(ok) => SerializableResult::Ok(ok),
                Err(err) => SerializableResult::Error(err),
            },
            Err(err) => SerializableResult::Error(DomainError::BadGateway { error: err.to_string() }),
        };

        Self::respond(media,
                      DomainServerMessage::SetDesiredPlayStateResponse { request_id, result },
                      ctx);
    }

    fn respond(media: ResponseMedia, message: DomainServerMessage, ctx: &mut <Self as Actor>::Context) {
        let payload = match media {
            ResponseMedia::MsgPack => MsgPack.serialize(&message)
                                             .map_err(|err| DomainError::Serialization { error: err.to_string() }),
            ResponseMedia::Json => Json.serialize(&message)
                                       .map_err(|err| DomainError::Serialization { error: err.to_string() }),
        };

        match payload {
            Ok(payload) => match media {
                ResponseMedia::MsgPack => ctx.binary(payload),
                ResponseMedia::Json => {
                    if let Ok(txt) = String::from_utf8(payload) {
                        ctx.text(txt)
                    }
                }
            },
            Err(err) => {}
        };
    }
}

impl Actor for WebSocketActor {
    type Context = WebsocketContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.run_interval(Duration::from_secs(1), Self::update);

        let register_cmd = RegisterWebSocket { address: ctx.address(),
                                               id:      self.id, };

        SocketsSupervisor::from_registry().send(register_cmd)
                                          .into_actor(self)
                                          .then(|res, act, ctx| {
                                              if res.is_err() {
                                                  ctx.stop();
                                              }
                                              fut::ready(())
                                          })
                                          .wait(ctx);
    }
}

impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for WebSocketActor {
    fn handle(&mut self, msg: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
        match msg {
            Ok(ws::Message::Ping(msg)) => ctx.pong(&msg),
            Ok(ws::Message::Text(txt)) => match Json.deserialize::<DomainClientMessage>(txt.as_ref()) {
                Ok(request) => {
                    self.submit(request, ResponseMedia::Json, ctx);
                }
                Err(err) => {
                    warn!(%err, "Could not deserialize WebSocketCommand from web socket");
                    ctx.stop();
                }
            },
            Ok(ws::Message::Binary(bin)) => match MsgPack.deserialize::<DomainClientMessage>(&bin[..]) {
                Ok(request) => {
                    self.submit(request, ResponseMedia::MsgPack, ctx);
                }
                Err(err) => {
                    warn!(%err, "Could not deserialize WebSocketCommand from web socket");
                    ctx.stop();
                }
            },
            Err(err) => {
                warn!(%err, "Could not read from web socket");
                ctx.stop();
            }
            _ => (),
        }
    }
}

impl Handler<WebSocketSend> for WebSocketActor {
    type Result = ();

    fn handle(&mut self, msg: WebSocketSend, ctx: &mut Self::Context) {
        ctx.binary(msg.bytes);
    }
}

impl Handler<NotifyTaskSecurity> for WebSocketActor {
    type Result = ();

    fn handle(&mut self, mut msg: NotifyTaskSecurity, ctx: &mut Self::Context) -> Self::Result {
        if let Some(secure_key) = self.secure_key.get(&msg.session_id) {
            match msg.security.remove(secure_key) {
                Some(security) => {
                    self.security.insert(msg.session_id, security);
                }
                None => {
                    self.security.remove(&msg.session_id);
                }
            }
        } else {
            self.security.remove(&msg.session_id);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WebSocketId(u64);

static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(0);

fn get_next_socket_id() -> WebSocketId {
    WebSocketId(NEXT_SOCKET_ID.fetch_add(1, SeqCst))
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
struct WebSocketMembership {
    secure_key:    SecureKey,
    web_socket_id: WebSocketId,
}

pub fn init() {
    let _ = SocketsSupervisor::from_registry();
}
