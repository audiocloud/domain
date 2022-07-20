use actix::spawn;
use clap::Args;
use nats_aflowt::Connection;
use once_cell::sync::OnceCell;

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

pub async fn init(opts: NatsOpts) -> anyhow::Result<()> {
    let client = nats_aflowt::connect(&opts.nats_url).await?;
    let ac_evt = client.subscribe("ac.inst.*.*.*").await?;
    let rpr_evt = client.subscribe("ac.rpr.*.*").await?;

    Ok(())
}
