use std::env;

use actix_web::middleware::Logger;
use actix_web::{App, HttpServer};
use clap::Parser;
use tracing::*;

use audiocloud_domain_server::{data, rest_api, service, web_sockets};

#[derive(Parser)]
struct Opts {
    #[clap(short, long, env, default_value = "7200")]
    port: u16,

    #[clap(short, long, env, default_value = "0.0.0.0")]
    bind: String,

    #[clap(flatten)]
    db: data::DataOpts,

    #[clap(flatten)]
    media: service::media::MediaOpts,

    #[clap(flatten)]
    cloud: service::cloud::CloudOpts,
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

    let boot = service::cloud::init(opts.cloud).await?;

    let event_base = boot.event_base;

    debug!(?event_base, "Event base");

    data::init(opts.db, boot).await?;

    debug!("Boot data initialized");

    service::media::init(opts.media)?;

    debug!("Media");

    service::instance::init();

    debug!("Instances");

    service::session::init();

    debug!("Sessions");

    service::cloud::spawn_command_listener(event_base as i64).await?;

    debug!("Command listener");

    service::session::become_online();

    debug!("Becoming online");

    info!(bind = opts.bind,
          port = opts.port,
          " ==== AudioCloud Domain server ==== ");

    // create actix
    HttpServer::new(|| {
        App::new().wrap(Logger::default())
                  .configure(rest_api::configure)
                  .configure(web_sockets::configure)
    }).bind((opts.bind.as_str(), opts.port))?
      .run()
      .await?;

    Ok(())
}
