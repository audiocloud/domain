use actix::Handler;
use itertools::Itertools;
use tracing::*;

use audiocloud_api::domain::streaming::DomainServerMessage;
use audiocloud_api::{AppTaskId, TaskEvent, TaskPermissions, Timestamped};

use crate::sockets::supervisor::SupervisedClient;
use crate::sockets::SocketsSupervisor;
use crate::tasks::messages::NotifyStreamingPacket;
use crate::ResponseMedia;

impl Handler<NotifyStreamingPacket> for SocketsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyStreamingPacket, ctx: &mut Self::Context) -> Self::Result {
        // TODO: at some point we may want a lookup from app tasks to clients
        for client in self.clients.values() {
            if self.client_can_on_task(client, &msg.task_id, TaskPermissions::can_audio) {
                let best_socket = client.sockets
                                        .values()
                                        .filter(|socket| *socket.init_complete.value())
                                        .filter(|socket| socket.is_valid(self.opts.socket_drop_timeout))
                                        .sorted_by_key(|socket| socket.score())
                                        .next();

                if let Some(socket) = best_socket {
                    let event = TaskEvent::StreamingPacket { packet: { msg.packet.clone() }, };
                    let msg = DomainServerMessage::TaskEvent { task_id: { msg.task_id.clone() },
                                                               event:   { event }, };

                    self.send_to_socket(socket, msg, ResponseMedia::MsgPack, ctx);
                }
            }
        }
    }
}

impl SocketsSupervisor {
    pub fn client_can_on_task(&self,
                              client: &SupervisedClient,
                              task_id: &AppTaskId,
                              predicate: impl Fn(&TaskPermissions) -> bool)
                              -> bool {
        client.memberships
              .get(task_id)
              .and_then(|secure_key| {
                  self.security
                      .get(task_id)
                      .and_then(|task_security| task_security.security.get(secure_key))
                      .map(predicate)
              })
              .unwrap_or_default()
    }
}
