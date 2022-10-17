use std::time::{Duration, Instant};

use actix::{Actor, Addr, Context, ContextFutureSpawner, Handler, WrapFuture};
use derive_more::IsVariant;
use futures::FutureExt;
use nanoid::nanoid;
use tracing::*;

use audiocloud_api::domain::streaming::DomainServerMessage;
use audiocloud_api::{ClientSocketId, Codec, MsgPack, SocketId, Timestamped};

use crate::sockets::supervisor::SupervisedClient;
use crate::sockets::web_rtc::WebRtcActor;
use crate::sockets::web_sockets::WebSocketActor;
use crate::sockets::{Disconnect, SendToClient, SocketReceived, SocketSend, SocketsSupervisor};
use crate::ResponseMedia;

#[derive(Debug)]
pub struct SupervisedSocket {
    pub actor_addr:    SocketActorAddr,
    pub last_pong_at:  Instant,
    pub init_complete: Timestamped<bool>,
}

impl SupervisedSocket {
    pub(crate) fn score(&self) -> usize {
        match self.actor_addr {
            SocketActorAddr::WebRtc(_) => 10,
            SocketActorAddr::WebSocket(_) => 1,
        }
    }
}

impl Drop for SupervisedSocket {
    fn drop(&mut self) {
        match &self.actor_addr {
            SocketActorAddr::WebRtc(socket) => socket.do_send(Disconnect),
            SocketActorAddr::WebSocket(socket) => socket.do_send(Disconnect),
        };
    }
}

#[derive(Clone, Debug, IsVariant)]
pub enum SocketActorAddr {
    WebRtc(Addr<WebRtcActor>),
    WebSocket(Addr<WebSocketActor>),
}

impl SupervisedSocket {
    pub fn is_valid(&self, socket_drop_timeout: u64) -> bool {
        self.last_pong_at.elapsed() < Duration::from_millis(socket_drop_timeout)
        && match &self.actor_addr {
            SocketActorAddr::WebRtc(addr) => addr.connected(),
            SocketActorAddr::WebSocket(addr) => addr.connected(),
        }
    }
}

impl SocketsSupervisor {
    pub(crate) fn send_to_socket_by_id(&mut self,
                                       id: &ClientSocketId,
                                       message: DomainServerMessage,
                                       media: ResponseMedia,
                                       ctx: &mut <Self as Actor>::Context)
                                       -> anyhow::Result<()> {
        debug!(?message, %id, "send");

        match self.clients.get(&id.client_id) {
            None => {}
            Some(client) => match client.sockets.get(&id.socket_id) {
                None => warn!(%id, ?message, "Socket not found, dropping message"),
                Some(socket) => self.send_to_socket(socket, message, media, ctx)?,
            },
        }

        Ok(())
    }

    pub(crate) fn send_to_socket(&self,
                                 socket: &SupervisedSocket,
                                 message: DomainServerMessage,
                                 media: ResponseMedia,
                                 ctx: &mut Context<SocketsSupervisor>)
                                 -> anyhow::Result<()> {
        let cmd = match media {
            ResponseMedia::MsgPack => SocketSend::Bytes(MsgPack.serialize(&message)?.into()),
            ResponseMedia::Json => SocketSend::Text(serde_json::to_string(&message)?),
        };

        match &socket.actor_addr {
            SocketActorAddr::WebRtc(web_rtc) => {
                web_rtc.send(cmd).map(drop).into_actor(self).spawn(ctx);
            }
            SocketActorAddr::WebSocket(web_socket) => {
                web_socket.send(cmd).map(drop).into_actor(self).spawn(ctx);
            }
        }

        Ok(())
    }

    pub(crate) fn remove_socket(&mut self, id: &ClientSocketId) {
        for client in self.clients.get_mut(&id.client_id) {
            client.sockets.remove(&id.socket_id);
        }
    }
}

impl Handler<SocketReceived> for SocketsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: SocketReceived, ctx: &mut Self::Context) -> Self::Result {
        self.on_socket_message_received(msg, ctx);
    }
}

impl Handler<SendToClient> for SocketsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: SendToClient, ctx: &mut Self::Context) -> Self::Result {
        let SendToClient { client_id: socket_id,
                           message,
                           media, } = msg;
        // TODO: !!! implement me !!!
        // let _ = self.send_to_socket_by_id(&socket_id, message, media, ctx);
    }
}
