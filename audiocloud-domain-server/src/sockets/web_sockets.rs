#![allow(unused_variables)]

use actix::{
    Actor, ActorContext, ActorFutureExt, AsyncContext, ContextFutureSpawner, Handler, StreamHandler, WrapFuture,
};
use actix_web::{get, web, HttpRequest, Responder};
use actix_web_actors::ws;
use actix_web_actors::ws::WebsocketContext;
use futures::FutureExt;
use serde::Deserialize;
use tracing::*;

use audiocloud_api::newtypes::SecureKey;

use crate::sockets::messages::{RegisterWebSocket, SocketReceived, SocketSend};
use crate::sockets::{get_next_socket_id, get_sockets_supervisor, SocketId};

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(ws_handler);
}

#[derive(Deserialize)]
struct AuthParams {
    secure_key: SecureKey,
}

#[get("/ws")]
async fn ws_handler(req: HttpRequest, stream: web::Payload) -> impl Responder {
    let resp = ws::start(WebSocketActor { id: get_next_socket_id(), }, &req, stream);
    resp
}

#[derive(Debug)]
pub struct WebSocketActor {
    id: SocketId,
}

impl Actor for WebSocketActor {
    type Context = WebsocketContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let register_cmd = RegisterWebSocket { address: ctx.address(),
                                               id:      self.id.clone(), };

        get_sockets_supervisor().send(register_cmd)
                                .into_actor(self)
                                .map(|res, act, ctx| {
                                    if res.is_err() {
                                        warn!(id = %act.id, "Failed to register websocket actor, giving up");
                                        ctx.stop();
                                    }
                                })
                                .wait(ctx);
    }
}

impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for WebSocketActor {
    fn handle(&mut self, msg: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
        match msg {
            Ok(ws::Message::Ping(msg)) => ctx.pong(&msg),
            Ok(ws::Message::Text(text)) => {
                get_sockets_supervisor().send(SocketReceived::Text(self.id.clone(), text.to_string()))
                                        .map(drop)
                                        .into_actor(self)
                                        .spawn(ctx);
            }
            Ok(ws::Message::Binary(bytes)) => {
                get_sockets_supervisor().send(SocketReceived::Bytes(self.id.clone(), bytes))
                                        .map(drop)
                                        .into_actor(self)
                                        .spawn(ctx);
            }
            Err(error) => {
                warn!(%error, "WebSocket reported error");
                ctx.stop();
            }
            _ => (),
        }
    }
}

impl Handler<SocketSend> for WebSocketActor {
    type Result = ();

    fn handle(&mut self, msg: SocketSend, ctx: &mut Self::Context) {
        match msg {
            SocketSend::Bytes(bytes) => {
                ctx.binary(bytes);
            }
            SocketSend::Text(text) => {
                ctx.text(text);
            }
        }
    }
}
