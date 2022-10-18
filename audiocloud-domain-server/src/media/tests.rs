use std::env;
use std::path::PathBuf;
use std::time::Duration;

use actix::Actor;
use serde_json::json;
use tokio::time::sleep;
use uuid::Uuid;

use audiocloud_api::{AppId, AppMediaObjectId, DownloadFromDomain, MediaDownload, MediaObjectId};

use crate::db;
use crate::db::DataOpts;
use crate::media::download::Downloader;
use crate::media::DownloadJobId;

#[actix::test]
async fn test_download_success() -> anyhow::Result<()> {
    env::set_var("RUST_LOG", "debug");

    tracing_subscriber::fmt::init();

    let db = db::init(DataOpts::memory()).await?;

    let job_id = DownloadJobId::new();

    let client = reqwest::Client::new();

    let source = PathBuf::from("../README.md");

    let media_id = AppMediaObjectId::new(AppId::admin(), MediaObjectId::new("object-1".to_owned()));

    // monitor at https://requestbin.com/r/en1205p765d7rp

    let settings = DownloadFromDomain { url:        "https://en1205p765d7rp.x.pipedream.net".to_string(),
                                        notify_url: Some("https://en1205p765d7rp.x.pipedream.net".to_string()),
                                        context:    Some(json!({"context": "is here"})), };

    let download_info = MediaDownload { media_id: media_id.clone(),
                                        download: settings,
                                        state:    Default::default(), };

    let upload = Downloader::new(db.clone(), job_id, client, source, download_info)?;

    let addr = upload.start();

    for i in 0..1000 {
        sleep(Duration::from_millis(50)).await;
        if !addr.connected() {
            println!("Endded at loop {i}");
            break;
        }
    }

    // load state
    let maybe_file = db.fetch_media_by_id(&media_id).await?;

    println!("maybe_file: {maybe_file:#?}");

    Ok(())
}
