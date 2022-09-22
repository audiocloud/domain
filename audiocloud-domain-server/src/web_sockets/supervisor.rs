use std::collections::{HashMap, HashSet};
use std::time::Duration;

use actix::{Actor, Addr, AsyncContext, Context, Handler, Supervised, SystemService};
use anyhow::anyhow;
use bytes::Bytes;
use tracing::*;

use audiocloud_api::api::codec::{Codec, MsgPack};
use audiocloud_api::domain::WebSocketEvent;
use audiocloud_api::newtypes::{AppTaskId, SecureKey};
use audiocloud_api::common::task::TaskPermissions;

use crate::task::messages::{NotifyTaskDeleted, NotifyStreamingPacket, NotifyTaskSecurity};
use crate::web_sockets::messages::{LoginWebSocket, LogoutWebSocket, RegisterWebSocket, WebSocketSend};
use crate::web_sockets::{WebSocketActor, WebSocketId, WebSocketMembership};

#[derive(Default)]
pub struct SocketsSupervisor {
    web_sockets: HashMap<WebSocketId, Addr<WebSocketActor>>,
    membership:  HashMap<AppTaskId, HashSet<WebSocketMembership>>,
    security:    HashMap<AppTaskId, HashMap<SecureKey, TaskPermissions>>,
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
    type Result = anyhow::Result<TaskPermissions>;

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

impl Handler<NotifyTaskDeleted> for SocketsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyTaskDeleted, ctx: &mut Self::Context) -> Self::Result {
        if let Some(accesses) = self.membership.remove(&msg.session_id) {
            for access in accesses {
                self.web_sockets.remove(&access.web_socket_id);
            }
        }
        self.prune_unlinked_access();
    }
}

impl Handler<NotifyTaskSecurity> for SocketsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyTaskSecurity, ctx: &mut Self::Context) -> Self::Result {
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

impl Handler<NotifyStreamingPacket> for SocketsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyStreamingPacket, ctx: &mut Self::Context) -> Self::Result {
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
