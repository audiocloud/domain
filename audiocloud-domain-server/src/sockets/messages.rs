use actix::{Addr, Message};
use bytes::Bytes;

use audiocloud_api::{AppTaskId, SocketId, StreamingPacket};

use crate::sockets::web_rtc::WebRtcActor;
use crate::sockets::web_sockets::WebSocketActor;

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub enum SocketSend {
    Bytes(Bytes),
    Text(String),
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub enum SocketReceived {
    Bytes(SocketId, Bytes),
    Text(SocketId, String),
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct RegisterWebSocket {
    pub address: Addr<WebSocketActor>,
    pub id:      SocketId,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct RegisterWebRtcSocket {
    pub address: Addr<WebRtcActor>,
    pub id:      SocketId,
}
