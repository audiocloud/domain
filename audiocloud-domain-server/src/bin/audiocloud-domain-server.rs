use std::env;

use actix_web::{App, HttpServer};
use audiocloud_api::newtypes::DomainId;
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

    #[clap(short, long, env)]
    api_key: String,

    #[clap(short, long, env, default_value = "https://api.audiocloud.org")]
    api_url: String,
}

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    // the domain server is basically a bunch of timers and event handlers running on top of a mongodb database.

    let _ = dotenv::dotenv();

    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG",
                     "info,audiocloud_domain_server=debug,audiocloud_api=debug,actix_server=warn");
    }

    tracing_subscriber::fmt::init();

    let opts = Opts::parse();

    let domain_id = DomainId::new("distopik_hq".to_owned());

    data::init_data(opts.db, &domain_id).await?;

    info!(bind = opts.bind,
          port = opts.port,
          " ==== AudioCloud Domain server ==== ");

    // create actix
    HttpServer::new(|| App::new().configure(rest::configure)).bind((opts.bind.as_str(), opts.port))?
                                                             .run()
                                                             .await?;

    Ok(())
}
