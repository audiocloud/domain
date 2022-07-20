use std::env;

use actix_web::{App, HttpServer};
use clap::Parser;
use tracing::*;

use audiocloud_domain_server::data::DataOpts;
use audiocloud_domain_server::service::cloud;
use audiocloud_domain_server::{data, rest};

#[derive(Parser)]
struct Opts {
    #[clap(short, long, env, default_value = "7200")]
    port: u16,

    #[clap(short, long, env, default_value = "0.0.0.0")]
    bind: String,

    #[clap(flatten)]
    db: DataOpts,

    #[clap(flatten)]
    cloud: cloud::CloudOpts,
}

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    // the domain server is basically a bunch of timers and event handlers running on top of a mongodb database.

    let _ = dotenv::dotenv();

    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG",
                     "info,audiocloud_domain_server=debug,audiocloud_api=debug,actix_server=warn,rdkafka=debug");
    }

    tracing_subscriber::fmt::init();

    let opts = Opts::parse();

    let boot = cloud::init(opts.cloud).await?;

    let event_base = boot.event_base;

    data::init(opts.db, boot).await?;

    // ideally this should not resolve until we are even with the upstream database.
    cloud::spawn_command_listener(event_base as i64).await?;

    info!(bind = opts.bind,
          port = opts.port,
          " ==== AudioCloud Domain server ==== ");

    // create actix
    HttpServer::new(|| {
        App::new().wrap(tracing_actix_web::TracingLogger::default())
                  .configure(rest::configure)
    }).bind((opts.bind.as_str(), opts.port))?
      .run()
      .await?;

    Ok(())
}
