use actix::Handler;
use tracing::*;

use audiocloud_api::domain::streaming::DomainServerMessage;
use audiocloud_api::{TaskEvent, Timestamped};

use crate::sockets::SocketsSupervisor;
use crate::tasks::messages::NotifyStreamingPacket;
use crate::ResponseMedia;

impl Handler<NotifyStreamingPacket> for SocketsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyStreamingPacket, ctx: &mut Self::Context) -> Self::Result {
        if let Some(members) = self.task_socket_members.get(&msg.task_id) {
            let mut members =
                members.iter()
                       .filter_map(|socket| self.sockets.get(&socket.socket_id).filter(|socket| socket.is_valid()))
                       .collect::<Vec<_>>();

            members.sort_by_key(|socket| if socket.actor_addr.is_web_rtc() { 1 } else { 0 });

            match members.first() {
                None => {
                    warn!(task_id = %msg.task_id, "Could not publish packet for task, no connected sockets found")
                }
                Some(socket) => {
                    let event = TaskEvent::StreamingPacket { packet: msg.packet };
                    let event = DomainServerMessage::TaskEvent { task_id: { msg.task_id.clone() },
                                                                 event:   { event }, };

                    if let Err(error) = self.send_to_socket(socket, event, ResponseMedia::MsgPack, ctx) {
                        warn!(%error, "Could not publish packet");
                    }
                }
            }
        } else {
            warn!(task_id = %msg.task_id, "Could not publish packet for task, task does not exist")
        }
    }
}
