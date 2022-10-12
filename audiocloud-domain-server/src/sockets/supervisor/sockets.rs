use std::time::{Duration, Instant};

use actix::{Actor, Addr, Context, ContextFutureSpawner, Handler, WrapFuture};
use derive_more::IsVariant;
use futures::FutureExt;
use nanoid::nanoid;
use tracing::*;

use audiocloud_api::domain::streaming::DomainServerMessage;
use audiocloud_api::{Codec, MsgPack, SocketId};

use crate::sockets::web_rtc::WebRtcActor;
use crate::sockets::web_sockets::WebSocketActor;
use crate::sockets::{Disconnect, SocketReceived, SocketSend, SocketsSupervisor};
use crate::ResponseMedia;

#[derive(Debug)]
pub struct Socket {
    pub actor_addr:   SocketActorAddr,
    pub last_pong_at: Instant,
}

impl Drop for Socket {
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

impl Socket {
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
                                       socket_id: &SocketId,
                                       message: DomainServerMessage,
                                       media: ResponseMedia,
                                       ctx: &mut <Self as Actor>::Context)
                                       -> anyhow::Result<()> {
        match self.sockets.get(socket_id) {
            None => {
                warn!(?message, "Socket not found, dropping message");
            }
            Some(socket) => self.send_to_socket(socket, message, media, ctx)?,
        }

        Ok(())
    }

    pub(crate) fn send_to_socket(&self,
                                 socket: &Socket,
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
}

impl Handler<SocketReceived> for SocketsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: SocketReceived, ctx: &mut Self::Context) -> Self::Result {
        self.on_socket_message_received(msg, ctx);
    }
}
