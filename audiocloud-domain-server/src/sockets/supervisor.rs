#![allow(unused_variables)]

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use actix::{Actor, ActorFutureExt, Context, ContextFutureSpawner, Handler, Supervised, WrapFuture};
use tracing::*;

use audiocloud_api::api::codec::{Codec, MsgPack};
use audiocloud_api::domain::streaming::DomainServerMessage::PeerConnectionResponse;
use audiocloud_api::domain::streaming::{DomainClientMessage, PeerConnectionCreated};
use audiocloud_api::domain::DomainError;
use audiocloud_api::newtypes::AppTaskId;
use audiocloud_api::{PlayId, RequestId, SerializableResult, StreamingPacket, TaskSecurity, Timestamped};
use sockets::{Socket, SocketActorAddr};

use crate::sockets::web_rtc::{AddIceCandidate, WebRtcActor};
use crate::sockets::{get_next_socket_id, SocketId, SocketMembership, SocketsOpts};
use crate::ResponseMedia;

use super::messages::*;

mod packets;
mod sockets;
mod handle_task_events;
mod timers;

pub struct SocketsSupervisor {
    opts:                SocketsOpts,
    sockets:             HashMap<SocketId, Socket>,
    task_socket_members: HashMap<AppTaskId, HashSet<SocketMembership>>,
    security:            HashMap<AppTaskId, TaskSecurity>,
    packet_cache:        HashMap<AppTaskId, HashMap<PlayId, HashMap<u64, Timestamped<StreamingPacket>>>>,
}

struct SocketContext {
    socket_id:  SocketId,
    request_id: RequestId,
    media:      ResponseMedia,
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
               sockets:             { Default::default() },
               task_socket_members: { Default::default() },
               security:            { Default::default() },
               packet_cache:        { Default::default() }, }
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
                       let socket = Socket { actor_addr:   { SocketActorAddr::WebRtc(addr) },
                                             last_pong_at: { Instant::now() }, };

                       actor.sockets.insert(socket_id.clone(), socket);
                       let res = PeerConnectionCreated::Created { socket_id:          { socket_id },
                                                                  remote_description: { local_description }, };
                       SerializableResult::Ok(res)
                   }
                   Err(error) => {
                       warn!(%error, "Failed to create WebRTC actor");
                       SerializableResult::Error(DomainError::WebRTCError { error: error.to_string(), })
                   }
               };

               let response = PeerConnectionResponse { request_id: { request.request_id },
                                                       result:     { result }, };

               let _ = actor.send_to_socket_by_id(&request.socket_id, response, request.media, ctx);
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
            let _ = self.send_to_socket_by_id(&request.socket_id,
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

impl Supervised for SocketsSupervisor {
    fn restarting(&mut self, ctx: &mut <Self as Actor>::Context) {
        self.register_timers(ctx);
    }
}
