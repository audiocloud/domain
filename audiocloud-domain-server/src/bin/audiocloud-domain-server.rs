use std::env;

use actix_web::middleware::Logger;
use actix_web::{App, HttpServer};
use clap::{Args, Parser};
use tracing::*;

use audiocloud_domain_server::{config, db, events, rest_api, service, web_sockets};

#[derive(Parser)]
struct Opts {
    /// REST and WebSocket API port
    #[clap(short, long, env, default_value = "7200")]
    port: u16,

    /// REST and WebSocket API host
    #[clap(short, long, env, default_value = "0.0.0.0")]
    bind: String,

    /// Beginning of UDP port range to use for WebRTC (inclusive)
    #[clap(long, env, default_value = "30000")]
    web_rtc_port_min: u16,

    /// End of UDP port range to use for WebRTC (inclusivec)
    #[clap(long, env, default_value = "40000")]
    web_rtc_port_max: u16,

    #[clap(flatten)]
    db: db::DataOpts,

    #[clap(flatten)]
    media: audiocloud_domain_server::media::MediaOpts,

    #[clap(flatten)]
    config: config::ConfigOpts,
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

    debug!("Loading config");

    let config = config::init(opts.config).await?;

    debug!("Initializing database");

    let db = db::init(opts.db).await?;

    debug!("Boot data initialized");

    audiocloud_domain_server::media::init(opts.media, db.clone()).await?;

    debug!(" ✔ Media");

    service::instance::init(config.clone()).await?;

    debug!(" ✔ Instances");

    audiocloud_domain_server::task::init(db.clone());

    debug!(" ✔ Tasks");

    debug!("Initializing distributed commands / events");

    events::init(config.command_source.clone(), config.event_sink.clone()).await?;

    debug!("Going online");

    audiocloud_domain_server::task::become_online();

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
