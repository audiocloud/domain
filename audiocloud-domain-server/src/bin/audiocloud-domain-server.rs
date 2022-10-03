use std::env;

use actix_web::middleware::Logger;
use actix_web::{App, HttpServer};
use clap::Parser;
use tracing::*;

use audiocloud_domain_server::{config, db, events, fixed_instances, media, rest_api, sockets, tasks};

#[derive(Parser)]
struct Opts {
    /// REST and WebSocket API port
    #[clap(short, long, env, default_value = "7200")]
    port: u16,

    /// REST and WebSocket API host
    #[clap(short, long, env, default_value = "0.0.0.0")]
    bind: String,

    #[clap(flatten)]
    db: db::DataOpts,

    #[clap(flatten)]
    media: media::MediaOpts,

    #[clap(flatten)]
    config: config::ConfigOpts,

    #[clap(flatten)]
    sockets: sockets::SocketsOpts,
}

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    // the domain server is basically a bunch of timers and event handlers running on top of an sqlite database.

    let _ = dotenv::dotenv();

    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG",
                     "info,audiocloud_domain_server=debug,audiocloud_api=debug,actix_server=warn,rdkafka=debug");
    }

    tracing_subscriber::fmt::init();

    let opts = Opts::parse();

    debug!(source = ?opts.config.config_source, "Loading config");

    let cfg = config::init(opts.config).await?;

    debug!("Initializing database");

    let db = db::init(opts.db).await?;

    media::init(opts.media, db.clone()).await?;

    debug!(" ✔ Media");

    let routing = fixed_instances::init(&cfg, db.clone()).await?;

    debug!(" ✔ Instances");

    tasks::init(db.clone(), &cfg, routing)?;

    debug!(" ✔ Tasks");

    debug!("Initializing distributed commands / events");

    events::init(cfg.command_source.clone(), cfg.event_sink.clone()).await?;

    debug!(" ✔ Commands / Events");

    debug!("Going online");

    tasks::become_online();

    debug!(" ✔ Online");

    sockets::init(opts.sockets)?;

    debug!("✔ Sockets");

    info!(bind = opts.bind,
          port = opts.port,
          " ==== AudioCloud Domain server ==== ");

    // create actix
    HttpServer::new(|| {
        App::new().wrap(Logger::default())
                  .configure(rest_api::configure)
                  .configure(sockets::configure)
    }).bind((opts.bind.as_str(), opts.port))?
      .run()
      .await?;

    Ok(())
}
