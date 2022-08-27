use std::env;

use actix_web::{web, App, HttpServer};
use clap::Parser;
use tracing::*;

use audiocloud_media_server::config::Config;
use audiocloud_media_server::db::Db;
use audiocloud_media_server::service::get_media_service;
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

    service::init(db.clone(), &config);

    info!(" -- Service initialized");

    let mut service = get_media_service().lock().await;

    service.clean_stale_sessions().await?;
    service.restart_pending_uploads().await?;
    service.restart_pending_downloads().await?;

    drop(service);

    HttpServer::new({
        let web_db = web::Data::new(db);
        move || {
            App::new().app_data(web_db.clone())
                      .service(web::scope("/v1").configure(rest_api::rest_api))
                      .wrap(actix_web::middleware::Logger::default())
        }
    }).bind((config.bind.as_str(), config.port))?
      .run()
      .await?;

    Ok(())
}
