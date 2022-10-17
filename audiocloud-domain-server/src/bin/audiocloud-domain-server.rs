use actix_web::middleware::Logger;
use actix_web::{web, App, HttpServer};
use clap::Parser;
use tracing::*;

use audiocloud_domain_server::{
    config, db, events, fixed_instances, media, models, nats, o11y, rest_api, sockets, tasks,
};

#[derive(Parser)]
struct Opts {
    /// REST and WebSocket API port
    #[clap(short, long, env, default_value = "7200")]
    port: u16,

    /// REST and WebSocket API host
    #[clap(short, long, env, default_value = "0.0.0.0")]
    bind: String,

    /// NATS URL
    #[clap(long, env, default_value = "nats://localhost:4222")]
    nats_url: String,

    #[clap(flatten)]
    db: db::DataOpts,

    #[clap(flatten)]
    media: media::MediaOpts,

    #[clap(flatten)]
    config: config::ConfigOpts,

    #[clap(flatten)]
    sockets: sockets::SocketsOpts,

    #[clap(flatten)]
    tasks: tasks::TaskOpts,

    #[clap(flatten)]
    rest: rest_api::RestOpts,

    #[clap(flatten)]
    o11y: o11y::O11yOpts,
}

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    // the domain server is basically a bunch of timers and event handlers running on top of an sqlite database.

    let _ = dotenv::dotenv();

    let opts = Opts::parse();

    let _o11y_guard = o11y::init(&opts.o11y)?;

    info!(source = %opts.config.describe(), "Loading config");

    let cfg = config::init(opts.config).await?;

    info!("Initializing...");

    let db = db::init(opts.db).await?;

    info!(" ⚡ Database");

    let _nats_guard = nats::init(&opts.nats_url).await?;

    info!(" ⚡ NATS");

    models::init(&cfg, db.clone()).await?;

    info!(" ⚡ Models");

    media::init(opts.media, db.clone()).await?;

    info!(" ⚡ Media");

    let routing = fixed_instances::init(&cfg, db.clone()).await?;

    info!(" ⚡ Instances");

    tasks::init(db.clone(), &opts.tasks, &cfg, routing)?;

    info!(" ⚡ Tasks (Offline)");

    events::init(cfg.command_source.clone(), cfg.event_sink.clone()).await?;

    info!(" ⚡ Cloud Events");

    tasks::become_online().await?;

    info!(" ⚡ Tasks (Online)");

    sockets::init(opts.sockets)?;

    info!(" ⚡ Sockets");

    info!(bind = opts.bind,
          port = opts.port,
          " ==== AudioCloud Domain server ==== ");

    let rest_opts = web::Data::new(opts.rest.clone());
    if rest_opts.rest_auth_strategy.is_development() {
        warn!("*** development authentication strategy enabled! ***");
    }

    // create actix
    HttpServer::new(move || {
        App::new().wrap(Logger::default())
                  .app_data(rest_opts.clone())
                  .configure(rest_api::configure)
                  .configure(sockets::configure)
    }).bind((opts.bind.as_str(), opts.port))?
      .run()
      .await?;

    Ok(())
}
