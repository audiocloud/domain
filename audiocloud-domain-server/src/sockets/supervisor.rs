#![allow(unused_variables)]

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use actix::{
    Actor, ActorFutureExt, Addr, AsyncContext, Context, ContextFutureSpawner, Handler, Supervised, WrapFuture,
};
use bytes::Bytes;
use futures::{FutureExt, SinkExt};
use nanoid::nanoid;
use tracing::*;

use audiocloud_api::api::codec::{Codec, MsgPack};
use audiocloud_api::common::task::TaskPermissions;
use audiocloud_api::domain::streaming::DomainServerMessage::PeerConnectionResponse;
use audiocloud_api::domain::streaming::{DomainClientMessage, DomainServerMessage, PeerConnectionCreated};
use audiocloud_api::domain::DomainError;
use audiocloud_api::newtypes::{AppTaskId, SecureKey};
use audiocloud_api::{RequestId, SerializableResult, TaskSecurity};

use crate::sockets::web_rtc::{AddIceCandidate, WebRtcActor};
use crate::sockets::web_sockets::WebSocketActor;
use crate::sockets::{get_next_socket_id, SocketId, SocketMembership, SocketsOpts};
use crate::tasks::messages::{NotifyStreamingPacket, NotifyTaskDeleted, NotifyTaskSecurity};
use crate::ResponseMedia;

use super::messages::*;

pub struct SocketsSupervisor {
    opts:                SocketsOpts,
    sockets:             HashMap<SocketId, Socket>,
    task_socket_members: HashMap<AppTaskId, HashSet<SocketMembership>>,
    security:            HashMap<AppTaskId, TaskSecurity>,
}

#[derive(Debug)]
struct Socket {
    actor_addr:   SocketActorAddr,
    last_pong_at: Instant,
}

#[derive(Clone, Debug)]
enum SocketActorAddr {
    WebRtc(Addr<WebRtcActor>),
    WebSocket(Addr<WebSocketActor>),
}

struct SocketContext {
    socket_id:  SocketId,
    request_id: RequestId,
    media:      ResponseMedia,
}

impl Socket {
    pub fn is_valid(&self) -> bool {
        self.last_pong_at.elapsed() < Duration::from_secs(5)
        && match &self.actor_addr {
            SocketActorAddr::WebRtc(addr) => addr.connected(),
            SocketActorAddr::WebSocket(addr) => addr.connected(),
        }
    }
}

impl Actor for SocketsSupervisor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.restarting(ctx);
    }
}

impl SocketsSupervisor {
    pub fn new(opts: SocketsOpts) -> Self {
        Self { opts:                { opts },
               sockets:             Default::default(),
               task_socket_members: Default::default(),
               security:            Default::default(), }
    }

    fn cleanup_stale_sockets(&mut self, ctx: &mut Context<Self>) {
        self.sockets.retain(|_, socket| socket.is_valid());
        self.prune_unlinked_access();
    }

    fn ping_active_sockets(&mut self, ctx: &mut Context<Self>) {
        if let Ok(ping) = MsgPack.serialize(&DomainServerMessage::Ping { challenge: nanoid!() }) {
            let ping = Bytes::from(ping);

            for socket in self.sockets.values() {
                let pong = SocketSend::Bytes(ping.clone());
                match &socket.actor_addr {
                    SocketActorAddr::WebRtc(web_rtc) => {
                        web_rtc.send(pong).map(drop).into_actor(self).spawn(ctx);
                    }
                    SocketActorAddr::WebSocket(web_socket) => {
                        web_socket.send(pong).map(drop).into_actor(self).spawn(ctx);
                    }
                }
            }
        }
    }

    fn prune_unlinked_access(&mut self) {
        for (session_id, access) in self.task_socket_members.iter_mut() {
            access.retain(|access| self.sockets.contains_key(&access.socket_id));
        }

        self.task_socket_members.retain(|_, access| !access.is_empty());
    }

    fn socket_received(&mut self, message: SocketReceived, ctx: &mut <Self as Actor>::Context) {
        let (request, socket_id, use_json) = match message {
            SocketReceived::Bytes(socket_id, bytes) => match MsgPack.deserialize::<DomainClientMessage>(bytes.as_ref())
            {
                Ok(request) => (request, socket_id, false),
                Err(error) => {
                    warn!(%error, %socket_id, "Failed to decode message, dropping socket");
                    self.sockets.remove(&socket_id);
                    return;
                }
            },
            SocketReceived::Text(socket_id, text) => match serde_json::from_str(&text) {
                Ok(request) => (request, socket_id, true),
                Err(error) => {
                    warn!(%error, %socket_id, "Failed to decode message, dropping socket");
                    self.sockets.remove(&socket_id);
                    return;
                }
            },
        };

        let socket = match self.sockets.get_mut(&socket_id) {
            None => {
                warn!(%socket_id, "Received message from unknown socket, dropping message");
                return;
            }
            Some(socket) => socket,
        };

        match request {
            DomainClientMessage::RequestSetDesiredPlayState { .. } => {}
            DomainClientMessage::RequestModifyTaskSpec { .. } => {}
            DomainClientMessage::RequestPeerConnection { request_id,
                                                         description, } => {
                let request = SocketContext { socket_id,
                                              request_id,
                                              media: ResponseMedia::MsgPack };
                self.request_peer_connection(request, description, ctx);
            }
            DomainClientMessage::SubmitPeerConnectionCandidate { request_id,
                                                                 socket_id: rtc_socket_id,
                                                                 candidate, } => {
                let request = SocketContext { socket_id,
                                              request_id,
                                              media: ResponseMedia::Json };
                self.submit_peer_connection_candidate(request, rtc_socket_id, candidate, ctx);
            }
            DomainClientMessage::RequestAttachToTask { .. } => {}
            DomainClientMessage::RequestDetachFromTask { .. } => {}
            DomainClientMessage::Pong { challenge, response } => {
                socket.last_pong_at = Instant::now();
            }
        }
    }

    fn send_to_socket(&mut self,
                      socket_id: &SocketId,
                      message: DomainServerMessage,
                      media: ResponseMedia,
                      ctx: &mut <Self as Actor>::Context)
                      -> anyhow::Result<()> {
        match self.sockets.get(socket_id) {
            None => {
                warn!(?message, "Socket not found, dropping message");
            }
            Some(socket) => {
                let cmd = match media {
                    ResponseMedia::MsgPack => SocketSend::Text(serde_json::to_string(&message)?),
                    ResponseMedia::Json => SocketSend::Bytes(MsgPack.serialize(&message)?.into()),
                };

                match &socket.actor_addr {
                    SocketActorAddr::WebRtc(web_rtc) => {
                        web_rtc.send(cmd).map(drop).into_actor(self).spawn(ctx);
                    }
                    SocketActorAddr::WebSocket(web_socket) => {
                        web_socket.send(cmd).map(drop).into_actor(self).spawn(ctx);
                    }
                }
            }
        }

        Ok(())
    }

    fn request_peer_connection(&mut self,
                               request: SocketContext,
                               remote_description: String,
                               ctx: &mut Context<SocketsSupervisor>) {
        let socket_id = get_next_socket_id();
        let opts = self.opts.clone();

        let fut = {
            let socket_id = socket_id.clone();
            async move { WebRtcActor::new(socket_id, remote_description, &opts.web_rtc).await }
        };

        fut.into_actor(self)
           .map(move |res, actor, ctx| {
               let result = match res {
                   Ok((addr, local_description)) => {
                       let addr = addr.start();
                       let socket = Socket { actor_addr:   SocketActorAddr::WebRtc(addr),
                                             last_pong_at: Instant::now(), };

                       actor.sockets.insert(socket_id.clone(), socket);
                       SerializableResult::Ok(PeerConnectionCreated::Created { socket_id,
                                                                               remote_description: local_description })
                   }
                   Err(error) => {
                       warn!(%error, "Failed to create WebRTC actor");
                       SerializableResult::Error(DomainError::WebRTCError { error: error.to_string(), })
                   }
               };

               let _ = actor.send_to_socket(&request.socket_id,
                                            PeerConnectionResponse { request_id: request.request_id,
                                                                     result },
                                            request.media,
                                            ctx);
           })
           .spawn(ctx);
    }

    fn submit_peer_connection_candidate(&mut self,
                                        request: SocketContext,
                                        rtc_socket_id: SocketId,
                                        candidate: String,
                                        ctx: &mut Context<SocketsSupervisor>) {
        let result = match self.sockets.get(&rtc_socket_id) {
            None => {
                warn!(%rtc_socket_id, "Socket not found, dropping message");
                Some(SerializableResult::Error(DomainError::SocketNotFound { socket_id: rtc_socket_id, }))
            }
            Some(socket) => match socket.actor_addr {
                SocketActorAddr::WebRtc(ref addr) => {
                    addr.send(AddIceCandidate { candidate })
                        .into_actor(self)
                        .map(|res, actor, ctx| {})
                        .spawn(ctx);
                    None
                }
                SocketActorAddr::WebSocket(_) => {
                    warn!(%rtc_socket_id, "Socket is not a WebRTC socket, dropping message");
                    Some(SerializableResult::Error(DomainError::SocketNotFound { socket_id: rtc_socket_id, }))
                }
            },
        };

        if let Some(result) = result {
            let _ = self.send_to_socket(&request.socket_id,
                                        PeerConnectionResponse { request_id: request.request_id,
                                                                 result },
                                        request.media,
                                        ctx);
        }
    }
}

impl Handler<RegisterWebSocket> for SocketsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: RegisterWebSocket, ctx: &mut Self::Context) -> Self::Result {
        self.sockets.insert(msg.id,
                            Socket { actor_addr:   SocketActorAddr::WebSocket(msg.address),
                                     last_pong_at: Instant::now(), });
    }
}

impl Handler<SocketReceived> for SocketsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: SocketReceived, ctx: &mut Self::Context) -> Self::Result {
        self.socket_received(msg, ctx);
    }
}

impl Handler<NotifyTaskDeleted> for SocketsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyTaskDeleted, ctx: &mut Self::Context) -> Self::Result {
        if let Some(accesses) = self.task_socket_members.remove(&msg.task_id) {
            for access in accesses {
                self.sockets.remove(&access.socket_id);
            }
        }
        self.prune_unlinked_access();
    }
}

impl Handler<NotifyTaskSecurity> for SocketsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyTaskSecurity, ctx: &mut Self::Context) -> Self::Result {
        self.security.insert(msg.task_id.clone(), msg.security.clone());
    }
}

impl Handler<NotifyStreamingPacket> for SocketsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyStreamingPacket, ctx: &mut Self::Context) -> Self::Result {
        // TODO: broadcast on the best quality
    }
}

impl Supervised for SocketsSupervisor {
    fn restarting(&mut self, ctx: &mut <Self as Actor>::Context) {
        ctx.run_interval(Duration::from_millis(20), Self::cleanup_stale_sockets);
        ctx.run_interval(Duration::from_secs(1), Self::ping_active_sockets);
    }
}
