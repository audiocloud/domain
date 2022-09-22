use std::time::Duration;

use anyhow::anyhow;
use futures::{stream, Stream, StreamExt};
use nats_aflowt::{connect, Connection};
use once_cell::sync::OnceCell;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tracing::warn;

use audiocloud_api::{Codec, Json, MsgPack};

static NATS_CONNECTION: OnceCell<Connection> = OnceCell::new();

pub async fn init(nats_url: &str) -> anyhow::Result<()> {
    let conn = connect(nats_url).await?;
    NATS_CONNECTION.set(conn)
                   .map_err(|_| anyhow!("NATS_CONNECTION already initialized"))?;

    Ok(())
}

pub fn subscribe<M: DeserializeOwned, C: Codec>(subject: String, codec: C) -> impl Stream<Item = M> {
    let subscribe = {
        let subject = subject.clone();

        move |connection| connection.subscribe(&subject)
    };

    let gen_message_stream = {
        let subject = subject.clone();

        move |subscription| match subscription {
            Ok(subscription) => subscription.stream().boxed(),
            Err(error) => {
                warn!(%error, %subject, "Failed to subscribe");
                stream::empty().throttle(Duration::from_millis(100)).boxed()
            }
        }
    };

    let parse_messages = {
        let subject = subject.clone();

        move |message| match codec.decode(&message.payload) {
            Ok(message) => Some(message),
            Err(error) => {
                warn!(%error, %subject, "Failed to decode message");
                None
            }
        }
    };

    let connection = NATS_CONNECTION.get().expect("NATS_CONNECTION initialized");

    stream::unfold(connection, subscribe).flat_map(gen_message_stream)
                                         .filter_map(parse_messages)
}

pub fn subscribe_msgpack<M: DeserializeOwned>(subject: String) -> impl Stream<Item = M> {
    subscribe(subject, MsgPack)
}

pub async fn publish<M: Serialize, C: Codec>(subject: &str, codec: C, message: M) -> anyhow::Result<()> {
    let connection = NATS_CONNECTION.get()
                                    .ok_or_else(|| anyhow!("NATS_CONNECTION initialized"))?;

    let message = codec.serialize(&message)?;
    connection.publish(&subject, &message).await?;

    Ok(())
}

pub async fn request<R: DeserializeOwned, M: Serialize, C: Codec>(subject: &str,
                                                                  codec: C,
                                                                  message: M)
                                                                  -> anyhow::Result<R> {
    let connection = NATS_CONNECTION.get()
                                    .ok_or_else(|| anyhow!("NATS_CONNECTION initialized"))?;
    let message = codec.serialize(&message)?;
    let reply = connection.request(&subject, &message).await?;
    Ok(codec.deserialize(&reply.payload)?)
}

pub async fn request_json<R: DeserializeOwned, M: Serialize>(subject: &str, message: M) -> anyhow::Result<R> {
    request(subject, Json, message).await
}
