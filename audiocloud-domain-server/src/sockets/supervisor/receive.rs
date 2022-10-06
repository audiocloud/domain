use std::convert::identity;
use std::time::Instant;

use actix::{fut, Actor, ActorFutureExt, ContextFutureSpawner, MailboxError, WrapFuture};
use futures::{FutureExt, TryFutureExt};
use tracing::*;

use audiocloud_api::domain::streaming::{DomainClientMessage, DomainServerMessage};
use audiocloud_api::domain::DomainError;
use audiocloud_api::{Codec, MsgPack};

use crate::sockets::supervisor::SocketContext;
use crate::sockets::{SocketMembership, SocketReceived, SocketsSupervisor};
use crate::tasks::{get_tasks_supervisor, messages};
use crate::{to_serializable, DomainSecurity, ResponseMedia};

impl SocketsSupervisor {
    pub fn on_socket_message_received(&mut self, message: SocketReceived, ctx: &mut <Self as Actor>::Context) {
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

        let response_media = if use_json {
            ResponseMedia::Json
        } else {
            ResponseMedia::MsgPack
        };

        match request {
            DomainClientMessage::RequestModifyTaskSpec { request_id,
                                                         task_id,
                                                         modify_spec,
                                                         revision, } => {
                // TODO: get security
                let security = DomainSecurity::Cloud;
                get_tasks_supervisor().send(messages::ModifyTask { modify_spec,
                                                                   security,
                                                                   task_id,
                                                                   revision })
                                      .map_err(bad_gateway)
                                      .and_then(fut::ready)
                                      .into_actor(self)
                                      .map(move |res, actor, ctx| {
                                          let result = to_serializable(res);
                                          let result =
                                              DomainServerMessage::ModifyTaskSpecResponse { request_id, result };
                                          actor.send_to_socket_by_id(&socket_id, result, response_media, ctx);
                                      })
                                      .spawn(ctx);
            }
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
            DomainClientMessage::RequestAttachToTask { request_id,
                                                       task_id,
                                                       secure_key, } => {
                // TODO: validate if secure key is valid
                self.task_socket_members
                    .entry(task_id)
                    .or_default()
                    .insert(SocketMembership { secure_key, socket_id });
            }
            DomainClientMessage::RequestDetachFromTask { request_id, task_id } => {
                if let Some(sockets) = self.task_socket_members.get_mut(&task_id) {
                    sockets.retain(|membership| &membership.socket_id != &socket_id);
                }
            }
            DomainClientMessage::Pong { challenge, response } => {
                socket.last_pong_at = Instant::now();
            }
        }
    }
}

fn bad_gateway(error: MailboxError) -> DomainError {
    DomainError::BadGateway { error: error.to_string(), }
}
