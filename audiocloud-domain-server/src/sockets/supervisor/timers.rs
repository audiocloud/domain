use std::time::Duration;

use actix::{AsyncContext, Context};
use nanoid::nanoid;
use tracing::field::debug;
use tracing::*;

use audiocloud_api::domain::streaming::DomainServerMessage;

use crate::sockets::SocketsSupervisor;
use crate::ResponseMedia;

impl SocketsSupervisor {
    pub(crate) fn cleanup_stale_sockets(&mut self, ctx: &mut Context<Self>) {
        self.sockets.retain(|id, socket| {
                        if !socket.is_valid(self.opts.socket_drop_timeout) {
                            debug!(%id, "Supervisor cleaning up disconnected socket");
                            false
                        } else {
                            true
                        }
                    });

        self.prune_unlinked_access();
    }

    pub(crate) fn ping_active_sockets(&mut self, ctx: &mut Context<Self>) {
        for (socket_id, socket) in &self.sockets {
            if !socket.is_valid(self.opts.socket_drop_timeout) {
                continue;
            }

            if let Err(error) = self.send_to_socket(socket,
                                                    DomainServerMessage::Ping { challenge: nanoid!() },
                                                    ResponseMedia::MsgPack,
                                                    ctx)
            {
                warn!(%error, %socket_id, "Failed to ping socket");
            }
        }
    }

    pub(crate) fn prune_unlinked_access(&mut self) {
        for (session_id, access) in self.task_socket_members.iter_mut() {
            access.retain(|access| self.sockets.contains_key(&access.socket_id));
        }

        self.task_socket_members.retain(|_, access| !access.is_empty());
    }

    pub(crate) fn register_timers(&mut self, ctx: &mut Context<Self>) {
        ctx.run_interval(Duration::from_millis(20), Self::cleanup_stale_sockets);
        ctx.run_interval(Duration::from_millis(self.opts.socket_ping_interval), Self::ping_active_sockets);
    }
}
