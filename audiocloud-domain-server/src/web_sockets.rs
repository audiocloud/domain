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
use bytes::Bytes;
use maplit::hashmap;
use serde::Deserialize;
use tracing::error;
use web::Query;

use audiocloud_api::change::ModifySessionSpec;
use audiocloud_api::codec::{Codec, MsgPack};
use audiocloud_api::domain::{DomainSessionCommand, WebSocketCommand, WebSocketEvent};
use audiocloud_api::newtypes::{AppId, AppSessionId, SecureKey, SessionId};
use audiocloud_api::session::SessionSecurity;
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
async fn ws_handler(req: HttpRequest,
                    stream: web::Payload,
                    ids: web::Path<(AppId, SessionId)>,
                    Query(AuthParams { secure_key }): Query<AuthParams>)
                    -> impl Responder {
    let (app_id, session_id) = ids.into_inner();
    let app_session_id = AppSessionId::new(app_id, session_id);

    let resp = ws::start(WebSocketActor { id:         get_next_socket_id(),
                                          security:   hashmap! {},
                                          secure_key: hashmap! {},
                                          created_at: Instant::now(), },
                         &req,
                         stream);
    resp
}

#[derive(Debug)]
pub struct WebSocketActor {
    id:         WebSocketId,
    security:   HashMap<AppSessionId, SessionSecurity>,
    secure_key: HashMap<AppSessionId, SecureKey>,
    created_at: Instant,
}

impl WebSocketActor {
    fn update(&mut self, ctx: &mut <Self as Actor>::Context) {
        if self.security.is_empty() && self.created_at.elapsed().as_secs() > 10 {
            ctx.stop();
        }
    }

    fn login(&mut self, session_id: AppSessionId, secure_key: SecureKey, ctx: &mut <Self as Actor>::Context) {
        let login = LoginWebSocket { id:         self.id,
                                     secure_key: secure_key.clone(),
                                     session_id: session_id.clone(), };

        let send_login_error = |ctx: &mut <Self as Actor>::Context, session_id: AppSessionId, err| {
            match MsgPack.serialize(&WebSocketEvent::LoginError(session_id, err)) {
                Ok(bytes) => {
                    ctx.notify(WebSocketSend { bytes: Bytes::from(bytes), });
                }
                Err(err) => {
                    error!(%err, "Failed to serialize LoginError");
                }
            }
        };

        let send_login_success = |ctx: &mut <Self as Actor>::Context, session_id: AppSessionId| {
            match MsgPack.serialize(&WebSocketEvent::LoginSuccess(session_id)) {
                Ok(bytes) => {
                    ctx.notify(WebSocketSend { bytes: Bytes::from(bytes), });
                }
                Err(err) => {
                    error!(%err, "Failed to serialize LoginSuccess");
                }
            }
        };

        self.secure_key.insert(session_id.clone(), secure_key.clone());

        SocketsSupervisor::from_registry().send(login)
                                          .into_actor(self)
                                          .map(move |res, act, ctx| match res {
                                              Ok(Ok(security)) => {
                                                  act.security.insert(session_id.clone(), security);
                                                  send_login_success(ctx, session_id);
                                              }
                                              Ok(Err(err)) => {
                                                  send_login_error(ctx, session_id, err.to_string());
                                              }
                                              Err(err) => {
                                                  send_login_error(ctx, session_id, err.to_string());
                                              }
                                          })
                                          .wait(ctx);
    }

    fn logout(&mut self, session_id: AppSessionId, ctx: &mut <Self as Actor>::Context) {
        let logout = LogoutWebSocket { id:         self.id,
                                       session_id: session_id.clone(), };

        self.secure_key.remove(&session_id);

        SocketsSupervisor::from_registry().send(logout)
                                          .into_actor(self)
                                          .map(move |res, act, ctx| {
                                              if res.is_ok() {
                                                  act.security.remove(&session_id);
                                                  act.created_at = Instant::now();
                                              }
                                          })
                                          .wait(ctx);
    }

    fn execute_session_command(&mut self, cmd: DomainSessionCommand, ctx: &mut <Self as Actor>::Context) {
        let send_session_error = |ctx: &mut <Self as Actor>::Context, session_id: AppSessionId, err| {
            match MsgPack.serialize(&WebSocketEvent::SessionError(session_id, err)) {
                Ok(bytes) => {
                    ctx.notify(WebSocketSend { bytes: Bytes::from(bytes), });
                }
                Err(err) => {
                    error!(%err, "Failed to serialize SessionError");
                }
            }
        };
        let session_id = cmd.get_session_id().clone();

        match &cmd {
            DomainSessionCommand::Modify { modifications, .. } => {
                for modification in modifications {
                    match modification {
                        ModifySessionSpec::SetConnectionParameterValues { .. }
                        | ModifySessionSpec::SetFixedInstanceParameterValues { .. }
                        | ModifySessionSpec::SetDynamicInstanceParameterValues { .. } => {
                            if !self.security_can(&session_id, |s| s.parameters) {
                                send_session_error(ctx,
                                                   session_id,
                                                   "You are now allowed to change parameters".to_string());
                                return;
                            }
                        }
                        modification => {
                            let kind = modification.get_kind();
                            send_session_error(ctx, session_id, format!("Modification {kind} is not supported"));

                            return;
                        }
                    }
                }
            }
            DomainSessionCommand::SetDesiredPlayState { desired_play_state, .. } => {
                if !self.security_can(&session_id, |s| s.transport) {
                    send_session_error(ctx,
                                       session_id,
                                       "You are not allowed to change the desired play state".to_string());

                    return;
                }
            }
            cmd => {
                let kind = cmd.get_kind();
                send_session_error(ctx, session_id, format!("Command {kind} is not supported"));

                return;
            }
        }

        // if we got here, our request is legit and we can forward it to the session
        let cmd = ExecuteSessionCommand { session_id: session_id.clone(),
                                          command:    cmd, };

        // we wait here to keep strict ordering of commands
        SessionsSupervisor::from_registry().send(cmd)
                                           .into_actor(self)
                                           .map(move |res, act, ctx| {
                                               if let Err(err) = res {
                                                   send_session_error(ctx, session_id, err.to_string());
                                               }
                                           })
                                           .wait(ctx);
    }

    fn security_can(&self, session_id: &AppSessionId, f: impl FnOnce(&SessionSecurity) -> bool) -> bool {
        if let Some(security) = self.security.get(session_id) {
            f(security)
        } else {
            false
        }
    }
}

impl Actor for WebSocketActor {
    type Context = ws::WebsocketContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let cmd = RegisterWebSocket { address: ctx.address(),
                                      id:      self.id, };

        ctx.run_interval(Duration::from_secs(1), Self::update);

        SocketsSupervisor::from_registry().send(cmd)
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
            Ok(ws::Message::Binary(bin)) => match MsgPack.deserialize::<WebSocketCommand>(&bin[..]) {
                Ok(cmd) => match cmd {
                    WebSocketCommand::Login(session_id, secure_key) => {
                        self.login(session_id, secure_key, ctx);
                    }
                    WebSocketCommand::Logout(session_id) => {
                        self.logout(session_id, ctx);
                    }
                    WebSocketCommand::Session(cmd) => {
                        self.execute_session_command(cmd, ctx);
                    }
                },
                Err(err) => {
                    error!(%err, "Could not deserialize WebSocketCommand from web socket");
                }
            },
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
