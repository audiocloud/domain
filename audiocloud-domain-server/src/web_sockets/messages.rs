use actix::{Addr, Message};
use bytes::Bytes;

use audiocloud_api::newtypes::{AppSessionId, SecureKey};
use audiocloud_api::session::SessionSecurity;

use crate::web_sockets::{WebSocketActor, WebSocketId};

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct WebSocketSend {
    pub bytes: Bytes,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct RegisterWebSocket {
    pub address: Addr<WebSocketActor>,
    pub id:      WebSocketId,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "anyhow::Result<SessionSecurity>")]
pub struct LoginWebSocket {
    pub id:         WebSocketId,
    pub session_id: AppSessionId,
    pub secure_key: SecureKey,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct LogoutWebSocket {
    pub id:         WebSocketId,
    pub session_id: AppSessionId,
}
