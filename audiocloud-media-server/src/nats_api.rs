use anyhow::anyhow;
use audiocloud_api::codec::{Codec, Json};
use audiocloud_api::media::MediaServiceEvent;
use once_cell::sync::OnceCell;

static NATS_CLIENT: OnceCell<nats_aflowt::Connection> = OnceCell::new();

pub async fn emit_nats_event(event: MediaServiceEvent) -> anyhow::Result<()> {
    let client = NATS_CLIENT.get()
                            .ok_or_else(|| anyhow!("NATS client not initialized"))?;

    let encoded = Json.serialize(&event)?;

    client.publish("ac.mdia.default.evt", encoded).await?;

    Ok(())
}
