use std::env;

use actix_web::{App, HttpServer};
use clap::Parser;
use tracing::*;

use audiocloud_domain_server::data::DataOpts;
use audiocloud_domain_server::{data, rest};

#[derive(Parser)]
struct Opts {
    #[clap(short, long, env, default_value = "7200")]
    port: u16,

    #[clap(short, long, env, default_value = "0.0.0.0")]
    bind: String,

    #[clap(flatten)]
    db: DataOpts,
}

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    // the domain server is basically a bunch of timers and event handlers running on top of an embedded sqlite database

    let _ = dotenv::dotenv();

    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG",
                     "info,audiocloud_domain_server=debug,audiocloud_api=debug,actix_server=warn");
    }

    tracing_subscriber::fmt::init();

    let opts = Opts::parse();

    data::init_data(opts.db).await?;

    info!(bind = opts.bind,
          port = opts.port,
          " ==== AudioCloud Domain server ==== ");

    // create actix
    HttpServer::new(|| App::new().configure(rest::configure)).bind((opts.bind.as_str(), opts.port))?
                                                             .run()
                                                             .await?;

    Ok(())
}
