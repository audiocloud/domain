use std::io;
use std::time::Duration;

use anyhow::anyhow;
use futures::{stream, Stream, StreamExt};
use nats_aflowt::{connect, Connection, Message, Subscription};
use once_cell::sync::OnceCell;
use serde::de::DeserializeOwned;
use serde::Serialize;

use audiocloud_api::{Codec, Json, MsgPack, Request};

static NATS_CONNECTION: OnceCell<Connection> = OnceCell::new();

pub async fn init(nats_url: &str) -> anyhow::Result<()> {
    let conn = connect(nats_url).await?;
    NATS_CONNECTION.set(conn)
                   .map_err(|_| anyhow!("NATS_CONNECTION already initialized"))?;

    Ok(())
}

pub fn subscribe<M: DeserializeOwned, C: Codec>(subject: String, codec: C) -> impl Stream<Item = M> {
    let conn = NATS_CONNECTION.get().expect("NATS_CONNECTION not initialized");

    stream::repeat(()).throttle(Duration::from_secs(1))
                      .then(move |_| conn.subscribe(&subject))
                      .filter_map(move |res: io::Result<Subscription>| async move { res.ok() })
                      .flat_map(move |sub: Subscription| sub.stream())
                      .filter_map(move |msg: Message| async move { codec.deserialize(msg.payload).ok() })
}

pub fn subscribe_msgpack<M: DeserializeOwned>(subject: String) -> impl Stream<Item = M> {
    subscribe(subject, MsgPack)
}

pub fn subscribe_json<M: DeserializeOwned>(subject: String) -> impl Stream<Item = M> {
    subscribe(subject, Json)
}

pub async fn publish<M: Serialize, C: Codec>(subject: &str, codec: C, message: M) -> anyhow::Result<()> {
    let connection = NATS_CONNECTION.get()
                                    .ok_or_else(|| anyhow!("NATS_CONNECTION initialized"))?;

    let message = codec.serialize(&message)?;
    connection.publish(&subject, &message).await?;

    Ok(())
}

pub async fn request<R: Request, C: Codec>(subject: &str,
                                           codec: C,
                                           req: R)
                                           -> anyhow::Result<<R as Request>::Response> {
    let connection = NATS_CONNECTION.get()
                                    .ok_or_else(|| anyhow!("NATS_CONNECTION initialized"))?;

    let req = codec.serialize(&req)?;
    let reply = connection.request(&subject, &req).await?;
    Ok(codec.deserialize(&reply.data)?)
}

pub async fn request_json<R: Request>(subject: &str, req: R) -> anyhow::Result<<R as Request>::Response> {
    request(subject, Json, req).await
}
