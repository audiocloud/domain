#![allow(unused_variables)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::SeqCst;
use std::time::{Duration, Instant};

use actix::{
    fut, Actor, ActorContext, ActorFutureExt, Addr, AsyncContext, Context, ContextFutureSpawner, Handler, Message,
    StreamHandler, Supervised, SystemService, WrapFuture,
};
use actix_web::{get, web, HttpRequest, Responder};
use actix_web_actors::ws;
use anyhow::anyhow;
use audiocloud_api::change::ModifySessionSpec;
use bytes::Bytes;
use maplit::hashmap;
use serde::Deserialize;
use tracing::error;

use audiocloud_api::codec::{Codec, MsgPack};
use audiocloud_api::domain::{DomainSessionCommand, WebSocketCommand, WebSocketEvent};
use audiocloud_api::newtypes::{AppId, AppSessionId, SecureKey, SessionId};
use audiocloud_api::session::SessionSecurity;

use crate::service::session::supervisor::SessionsSupervisor;
use crate::service::session::{
    ExecuteSessionCommand, NotifySessionDeleted, NotifySessionPacket, NotifySessionSecurity,
};

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
                    auth: web::Query<AuthParams>)
                    -> impl Responder {
    let (app_id, session_id) = ids.into_inner();
    let app_session_id = AppSessionId::new(app_id, session_id);
    let AuthParams { secure_key } = auth.into_inner();

    let resp = ws::start(WebSocketActor { id:         get_next_socket_id(),
                                          security:   hashmap! {},
                                          secure_key: hashmap! {},
                                          created_at: Instant::now(), },
                         &req,
                         stream);
    resp
}

struct WebSocketActor {
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

        self.secure_key.insert(session_id.clone(), secure_key.clone());

        SocketsSupervisor::from_registry().send(login)
                                          .into_actor(self)
                                          .map(move |res, act, ctx| match res {
                                              Ok(Ok(security)) => {
                                                  act.security.insert(session_id, security);
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
                        ModifySessionSpec::SetInputValues { .. }
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

#[derive(Message)]
#[rtype(result = "()")]
struct WebSocketSend {
    bytes: Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct WebSocketId(u64);

static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(0);

fn get_next_socket_id() -> WebSocketId {
    WebSocketId(NEXT_SOCKET_ID.fetch_add(1, SeqCst))
}

#[derive(Default)]
pub struct SocketsSupervisor {
    web_sockets: HashMap<WebSocketId, Addr<WebSocketActor>>,
    membership:  HashMap<AppSessionId, HashSet<WebSocketMembership>>,
    security:    HashMap<AppSessionId, HashMap<SecureKey, SessionSecurity>>,
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
struct WebSocketMembership {
    secure_key:    SecureKey,
    web_socket_id: WebSocketId,
}

impl Actor for SocketsSupervisor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.run_interval(Duration::from_secs(1), Self::update);
        self.restarting(ctx);
    }
}

impl SocketsSupervisor {
    fn update(&mut self, ctx: &mut Context<Self>) {
        self.web_sockets.retain(|_, socket| socket.connected());
        self.prune_unlinked_access();
    }

    fn prune_unlinked_access(&mut self) {
        for (session_id, access) in self.membership.iter_mut() {
            access.retain(|access| self.web_sockets.contains_key(&access.web_socket_id));
        }

        self.membership.retain(|_, access| !access.is_empty());
    }
}

impl Handler<RegisterWebSocket> for SocketsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: RegisterWebSocket, ctx: &mut Self::Context) -> Self::Result {
        self.web_sockets.insert(msg.id, msg.address);
    }
}

impl Handler<LoginWebSocket> for SocketsSupervisor {
    type Result = anyhow::Result<SessionSecurity>;

    fn handle(&mut self, msg: LoginWebSocket, ctx: &mut Self::Context) -> Self::Result {
        self.membership
            .entry(msg.session_id.clone())
            .or_insert_with(|| HashSet::new())
            .insert(WebSocketMembership { secure_key:    msg.secure_key.clone(),
                                          web_socket_id: msg.id, });

        Ok(self.security
               .get(&msg.session_id)
               .and_then(|sec| sec.get(&msg.secure_key))
               .ok_or_else(|| anyhow!("Could not find session security"))?
               .clone())
    }
}

impl Handler<LogoutWebSocket> for SocketsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: LogoutWebSocket, ctx: &mut Self::Context) -> Self::Result {
        if let Some(memberships) = self.membership.get_mut(&msg.session_id) {
            memberships.retain(|membership| membership.web_socket_id != msg.id);
        }
    }
}

impl Handler<NotifySessionDeleted> for SocketsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifySessionDeleted, ctx: &mut Self::Context) -> Self::Result {
        if let Some(accesses) = self.membership.remove(&msg.session_id) {
            for access in accesses {
                self.web_sockets.remove(&access.web_socket_id);
            }
        }
        self.prune_unlinked_access();
    }
}

impl Handler<NotifySessionSecurity> for SocketsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifySessionSecurity, ctx: &mut Self::Context) -> Self::Result {
        let old_security = self.security.insert(msg.session_id.clone(), msg.security.clone());
        if let Some(memberships) = self.membership.get(&msg.session_id) {
            for membership in memberships {
                if let Some(socket) = self.web_sockets.get(&membership.web_socket_id) {
                    socket.do_send(msg.clone());
                }
            }
        }
    }
}

impl Handler<NotifySessionPacket> for SocketsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifySessionPacket, ctx: &mut Self::Context) -> Self::Result {
        if let (Some(session_sockets), Some(session_security)) =
            (self.membership.get(&msg.session_id), self.security.get(&msg.session_id))
        {
            let event = WebSocketEvent::Packet(msg.session_id, msg.packet);
            let bytes = match MsgPack.serialize(&event) {
                Ok(bytes) => Bytes::from(bytes),
                Err(err) => {
                    error!(%err, "Failed to serialize a session packet");
                    return;
                }
            };

            for socket in session_sockets.iter() {
                if let Some(socket) = self.web_sockets.get(&socket.web_socket_id) {
                    socket.do_send(WebSocketSend { bytes: bytes.clone() });
                }
            }
        }
    }
}

impl Supervised for SocketsSupervisor {
    fn restarting(&mut self, ctx: &mut <Self as Actor>::Context) {}
}

impl SystemService for SocketsSupervisor {}

pub fn init() {
    let _ = SocketsSupervisor::from_registry();
}

#[derive(Message, Clone)]
#[rtype(result = "()")]
struct RegisterWebSocket {
    address: Addr<WebSocketActor>,
    id:      WebSocketId,
}

#[derive(Message, Clone)]
#[rtype(result = "anyhow::Result<SessionSecurity>")]
struct LoginWebSocket {
    id:         WebSocketId,
    session_id: AppSessionId,
    secure_key: SecureKey,
}

#[derive(Message, Clone)]
#[rtype(result = "()")]
struct LogoutWebSocket {
    id:         WebSocketId,
    session_id: AppSessionId,
}
