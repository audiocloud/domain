use actix::spawn;
use actix_web::http::header::q;
use clap::Args;
use nats_aflowt::Connection;
use once_cell::sync::OnceCell;

use audiocloud_api::audio_engine::AudioEngineCommand;
use audiocloud_api::codec::{Codec, MsgPack};
use audiocloud_api::error::SerializableResult;

#[derive(Args)]
pub struct NatsOpts {
    #[clap(long, short, env, default_value = "nats://localhost:4222")]
    pub nats_url: String,
}

static NATS_CLIENT: OnceCell<NatsClient> = OnceCell::new();

pub fn get_nats_client() -> &'static NatsClient {
    NATS_CLIENT.get().expect("NATS client not initialized")
}

pub struct NatsClient {
    connection: Connection,
}

impl NatsClient {
    pub async fn request_audio_engine(&self, engine_id: &str, request: AudioEngineCommand) -> anyhow::Result<()> {
        let encoded = MsgPack.serialize(&request)?;
        let topic = format!("ac.audio_engine.{}.cmd", engine_id);
        let response = self.connection.request(&topic, encoded).await?;
        MsgPack.deserialize::<SerializableResult>(&response.data)?.into()
    }
}

pub async fn init(opts: NatsOpts) -> anyhow::Result<()> {
    let client = nats_aflowt::connect(&opts.nats_url).await?;
    let ac_evt = client.subscribe("ac.inst.*.*.*").await?;
    let rpr_evt = client.subscribe("ac.engines.*.*").await?;

    Ok(())
}
