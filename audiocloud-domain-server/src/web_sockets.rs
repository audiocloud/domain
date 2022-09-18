#![allow(unused_variables)]

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::SeqCst;
use std::time::{Duration, Instant};

use actix::{
    fut, Actor, ActorContext, ActorFutureExt, AsyncContext, ContextFutureSpawner, Handler, StreamHandler,
    SystemService, WrapFuture,
};
use actix_web::{get, web, HttpRequest, Responder};
use actix_web_actors::ws;
use maplit::hashmap;
use serde::Deserialize;
use tracing::*;

use audiocloud_api::api::codec::{Codec, MsgPack};
use audiocloud_api::domain::{DomainSessionCommand, WebSocketCommand, WebSocketCommandEnvelope, WebSocketEvent};
use audiocloud_api::common::error::SerializableResult;
use audiocloud_api::newtypes::{AppTaskId, SecureKey};
use audiocloud_api::common::task::TaskPermissions;
use messages::{LoginWebSocket, LogoutWebSocket, RegisterWebSocket, WebSocketSend};
use supervisor::SocketsSupervisor;

use crate::service::session::messages::{ExecuteSessionCommand, NotifySessionSecurity};
use crate::service::session::supervisor::SessionsSupervisor;

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

    fn respond(ctx: &mut <Self as Actor>::Context, request_id: String, result: SerializableResult) {
        let response = WebSocketEvent::Response(request_id, result);
        match MsgPack.serialize(&response) {
            Ok(bytes) => ctx.binary(bytes),
            Err(err) => error!(%err, ?response, "Failed to serialize response"),
        };
    }

    fn login(&mut self,
             request_id: String,
             session_id: AppTaskId,
             secure_key: SecureKey,
             ctx: &mut <Self as Actor>::Context) {
        let login = LoginWebSocket { id:         self.id,
                                     secure_key: secure_key.clone(),
                                     session_id: session_id.clone(), };

        self.secure_key.insert(session_id.clone(), secure_key.clone());

        let supervisor = SocketsSupervisor::from_registry();
        supervisor.send(login)
                  .into_actor(self)
                  .map(move |res, act, ctx| match res {
                      Ok(Ok(security)) => {
                          act.security.insert(session_id.clone(), security);
                          act.security_updated_at = Instant::now();

                          Self::respond(ctx, request_id, SerializableResult::Ok(()));
                      }
                      Ok(Err(err)) => {
                          Self::respond(ctx,
                                        request_id,
                                        SerializableResult::Err { code:    400,
                                                                  message: err.to_string(), });
                      }
                      Err(err) => {
                          Self::respond(ctx,
                                        request_id,
                                        SerializableResult::Err { code:    500,
                                                                  message: err.to_string(), });
                      }
                  })
                  .wait(ctx);
    }

    fn logout(&mut self, request_id: String, session_id: AppTaskId, ctx: &mut <Self as Actor>::Context) {
        let logout = LogoutWebSocket { id:         self.id,
                                       session_id: session_id.clone(), };

        self.secure_key.remove(&session_id);

        let supervisor = SocketsSupervisor::from_registry();
        supervisor.send(logout)
                  .into_actor(self)
                  .map(move |res, act, ctx| match res {
                      Ok(_) => {
                          act.security.remove(&session_id);
                          act.security_updated_at = Instant::now();

                          Self::respond(ctx, request_id, SerializableResult::Ok(()));
                      }
                      Err(err) => {
                          Self::respond(ctx,
                                        request_id,
                                        SerializableResult::Err { code:    500,
                                                                  message: err.to_string(), });
                      }
                  })
                  .wait(ctx);
    }

    fn execute_session_command(&mut self,
                               request_id: String,
                               command: DomainSessionCommand,
                               ctx: &mut <Self as Actor>::Context) {
        let session_id = command.get_session_id().clone();
        match self.security.get(&session_id).cloned() {
            None => {
                Self::respond(ctx,
                              request_id,
                              SerializableResult::Err { code:    404,
                                                        message: "Session not logged in or not found".to_string(), });
            }
            Some(security) => {
                let supervisor = SessionsSupervisor::from_registry();
                let exec_cmd = ExecuteSessionCommand { session_id,
                                                       security,
                                                       command };
                supervisor.send(exec_cmd)
                          .into_actor(self)
                          .map(move |res, _, ctx| match res {
                              Ok(result) => match result {
                                  Ok(ok) => Self::respond(ctx, request_id, SerializableResult::Ok(ok)),
                                  Err(err) => {
                                      Self::respond(ctx,
                                                    request_id,
                                                    SerializableResult::Err { code:    400,
                                                                              message: err.to_string(), });
                                  }
                              },
                              Err(err) => {
                                  Self::respond(ctx,
                                                request_id,
                                                SerializableResult::Err { code:    500,
                                                                          message: err.to_string(), });
                              }
                          })
                          .spawn(ctx);
            }
        }
    }
}

impl Actor for WebSocketActor {
    type Context = ws::WebsocketContext<Self>;

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
            Ok(ws::Message::Binary(bin)) => match MsgPack.deserialize::<WebSocketCommandEnvelope>(&bin[..]) {
                Ok(envelope) => {
                    let WebSocketCommandEnvelope { request_id, command } = envelope;
                    match command {
                        WebSocketCommand::Login(session_id, secure_key) => {
                            self.login(request_id, session_id, secure_key, ctx);
                        }
                        WebSocketCommand::Logout(session_id) => {
                            self.logout(request_id, session_id, ctx);
                        }
                        WebSocketCommand::Session(cmd) => {
                            self.execute_session_command(request_id, cmd, ctx);
                        }
                    }
                }
                Err(err) => {
                    warn!(%err, "Could not deserialize WebSocketCommand from web socket");
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

impl Handler<NotifySessionSecurity> for WebSocketActor {
    type Result = ();

    fn handle(&mut self, mut msg: NotifySessionSecurity, ctx: &mut Self::Context) -> Self::Result {
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
