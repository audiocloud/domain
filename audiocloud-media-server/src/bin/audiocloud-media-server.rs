use std::env;

use actix_web::rt::{spawn, Runtime};
use actix_web::{web, App, HttpServer};
use clap::Parser;
use tracing::*;

use audiocloud_media_server::config::Config;
use audiocloud_media_server::db::Db;
use audiocloud_media_server::{rest_api, service};

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();

    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG",
                     "info,audiocloud_media_server=debug,audiocloud_api=debug,actix_server=warn,sled=debug");
    }

    tracing_subscriber::fmt::init();

    info!("Starting up");

    let config = Config::parse();

    info!(?config, " -- read config");

    let db = Db::new(&config.db).await?;

    info!(" -- DB initialized");

    service::init(db.clone());

    info!(" -- Service initialized");

    HttpServer::new({
        let web_db = web::Data::new(db);
        move || {
            App::new().app_data(web_db.clone())
                      .service(web::scope("/v1").configure(rest_api::rest_api))
        }
    }).bind((config.bind.as_str(), config.port))?
      .run()
      .await?;

    Ok(())
}
