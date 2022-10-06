use std::time::Duration;

use actix::{AsyncContext, Context};
use nanoid::nanoid;
use tracing::*;

use audiocloud_api::domain::streaming::DomainServerMessage;

use crate::sockets::SocketsSupervisor;
use crate::ResponseMedia;

impl SocketsSupervisor {
    pub(crate) fn cleanup_stale_sockets(&mut self, ctx: &mut Context<Self>) {
        self.sockets.retain(|_, socket| socket.is_valid());
        self.prune_unlinked_access();
    }

    pub(crate) fn ping_active_sockets(&mut self, ctx: &mut Context<Self>) {
        for (socket_id, socket) in &self.sockets {
            if let Err(error) = self.send_to_socket(socket,
                                                    DomainServerMessage::Ping { challenge: nanoid!() },
                                                    ResponseMedia::MsgPack,
                                                    ctx)
            {
                warn!(%error, %socket_id, "Failed to ping socket");
            }
        }
    }

    pub(crate) fn update_packet_cache(&mut self, ctx: &mut Context<Self>) {
        let packet_cache_max_retention = chrono::Duration::milliseconds(self.opts.packet_cache_max_retention_ms as i64);

        self.packet_cache.values_mut().for_each(|task_cache| {
                                          task_cache.values_mut().for_each(|play_id_cache| {
                                                                     play_id_cache.retain(|_, packet| {
                                                                                      packet.elapsed()
                                                                                      < packet_cache_max_retention
                                                                                  });
                                                                 });
                                          task_cache.retain(|_, play_id_cache| !play_id_cache.is_empty());
                                      });

        self.packet_cache.retain(|_, task_cache| !task_cache.is_empty());
    }

    pub(crate) fn prune_unlinked_access(&mut self) {
        for (session_id, access) in self.task_socket_members.iter_mut() {
            access.retain(|access| self.sockets.contains_key(&access.socket_id));
        }

        self.task_socket_members.retain(|_, access| !access.is_empty());
    }

    pub(crate) fn register_timers(&mut self, ctx: &mut Context<Self>) {
        ctx.run_interval(Duration::from_millis(20), Self::cleanup_stale_sockets);
        ctx.run_interval(Duration::from_millis(250), Self::update_packet_cache);
        ctx.run_interval(Duration::from_millis(1000), Self::ping_active_sockets);
    }
}
