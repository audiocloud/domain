use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use actix_web::rt::spawn;
use actix_web::rt::task::JoinHandle;
use flume::{Receiver, Sender};
use futures::lock::Mutex;
use futures::{FutureExt, TryStreamExt};
use once_cell::sync::OnceCell;
use serde_json::json;
use tokio::time::timeout;
use tokio_util::io::StreamReader;
use tracing::*;

use crate::config::Config;
use audiocloud_api::media::{
    DownloadFromDomain, MediaDownloadState, MediaObject, MediaServiceEvent, MediaUploadState, UploadToDomain,
};
use audiocloud_api::newtypes::AppMediaObjectId;
use audiocloud_api::time::Timestamped;

use crate::db::Db;
use crate::nats_api::emit_nats_event;

static MEDIA_SERVICE: OnceCell<Arc<Mutex<Service>>> = OnceCell::new();

/// Download a file from the domain media server and optionally notify the requester
///
/// # Arguments
///
/// * `id`: the ID of the file to be downloaded
/// * `source`: local path of the file to download
/// * `download`: details of the download request
///
/// returns: Result<(), Error>
#[instrument(skip(client), err)]
pub async fn download_from_domain(client: reqwest::Client,
                                  id: AppMediaObjectId,
                                  source: String,
                                  download: DownloadFromDomain)
                                  -> anyhow::Result<()> {
    client.put(&download.url)
          .body(tokio::fs::File::open(&source).await?)
          .send()
          .await?;

    if let Some(notify_url) = download.notify_url {
        client.post(&notify_url)
              .json(&json!({
                        "context": download.context,
                        "id": id,
                    }))
              .send()
              .await?;
    }

    Ok(())
}

/// Upload a file from a remote source to the media server and optionally notify the requester
///
/// # Arguments
///
/// * `id`: the ID of the file to be uploaded
/// * `destination`: local path of the file to overwrite
/// * `upload`: details of the upload request
///
/// returns: Result<(), Error>
#[instrument(skip(client), err)]
pub async fn upload_to_domain(client: reqwest::Client,
                              id: AppMediaObjectId,
                              destination: String,
                              upload: UploadToDomain)
                              -> anyhow::Result<()> {
    let mut file = tokio::fs::File::create(&destination).await?;

    let stream = client.get(&upload.url)
                       .send()
                       .await?
                       .bytes_stream()
                       .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err));

    let mut stream = StreamReader::new(stream);

    tokio::io::copy(&mut stream, &mut file).await?;

    if let Some(notify_url) = upload.notify_url {
        client.post(&notify_url)
              .json(&json!({
                        "context": upload.context,
                        "app_id": id.app_id,
                        "media_id": id.media_id
                    }))
              .send()
              .await?;
    }

    Ok(())
}

pub struct Service {
    db:        Db,
    client:    reqwest::Client,
    root_dir:  PathBuf,
    downloads: HashMap<AppMediaObjectId, Download>,
    uploads:   HashMap<AppMediaObjectId, Upload>,
    tx_status: Sender<JobStatus>,
}

struct Download {
    task:  Timestamped<JoinHandle<()>>,
    spec:  DownloadFromDomain,
    retry: usize,
}

struct Upload {
    task:  Timestamped<JoinHandle<()>>,
    spec:  UploadToDomain,
    retry: usize,
}

enum JobStatus {
    DownloadingFinished(AppMediaObjectId, anyhow::Result<()>),
    UploadingFinished(AppMediaObjectId, anyhow::Result<()>),
}

impl Service {
    pub fn new(db: Db, cfg: &Config) -> (JoinHandle<()>, Arc<Mutex<Self>>) {
        let (tx_status, rx_status) = flume::unbounded();
        let service = Arc::new(Mutex::new(Self { client: Default::default(),
                                                 root_dir: cfg.root_dir.clone(),
                                                 downloads: Default::default(),
                                                 uploads: Default::default(),
                                                 db,
                                                 tx_status }));

        (spawn(Service::update_task(rx_status, service.clone())), service)
    }

    pub fn relative_path(mut buf: PathBuf, id: &AppMediaObjectId) -> String {
        buf.push(id.app_id.as_str());
        buf.push(id.media_id.as_str());

        buf.to_string_lossy().to_string()
    }

    pub fn local_path(&self, id: &AppMediaObjectId) -> String {
        Self::relative_path(self.root_dir.clone(), id)
    }

    pub fn add_download(&mut self, id: AppMediaObjectId, download: DownloadFromDomain) {
        let source = self.local_path(&id);
        let handle = self.new_download_task(id.clone(), download.clone(), source);

        self.downloads.insert(id,
                              Download { task:  handle.into(),
                                         spec:  download,
                                         retry: 0, });
    }

    pub fn add_upload(&mut self, id: AppMediaObjectId, upload: UploadToDomain) {
        let destination = self.local_path(&id);
        let handle = self.new_upload_task(id.clone(), upload.clone(), destination);

        self.uploads.insert(id,
                            Upload { task:  handle.into(),
                                     spec:  upload,
                                     retry: 0, });
    }

    async fn update_task(rx_status: Receiver<JobStatus>, service: Arc<Mutex<Self>>) {
        loop {
            while let Ok(Ok(status)) = timeout(Duration::from_secs(1), rx_status.recv_async()).await {
                match status {
                    JobStatus::DownloadingFinished(id, res) => {
                        let mut service = service.lock().await;
                        let _ = service.downloading_finished(id, res).await;
                    }
                    JobStatus::UploadingFinished(id, res) => {
                        let mut service = service.lock().await;
                        let _ = service.uploading_finished(id, res).await;
                    }
                }
            }
        }
    }

    fn new_download_task(&self, id: AppMediaObjectId, download: DownloadFromDomain, source: String) -> JoinHandle<()> {
        let tx_status = self.tx_status.clone();

        spawn(download_from_domain(self.client.clone(), id.clone(), source, download.clone()).then(|res| async move {
                  let _ = tx_status.send_async(JobStatus::DownloadingFinished(id, res)).await;
              }))
    }

    fn new_upload_task(&mut self, id: AppMediaObjectId, upload: UploadToDomain, destination: String) -> JoinHandle<()> {
        let tx_status = self.tx_status.clone();

        spawn(upload_to_domain(self.client.clone(), id.clone(), destination, upload).then(|res| async move {
                  let _ = tx_status.send_async(JobStatus::UploadingFinished(id, res)).await;
              }))
    }

    async fn downloading_finished(&mut self, id: AppMediaObjectId, res: anyhow::Result<()>) -> anyhow::Result<()> {
        debug!(%id, ?res, "downloading finished");

        if let Some(mut download) = self.downloads.remove(&id) {
            match res {
                Ok(_) => {
                    self.db
                        .update_media_status(&id, |persisted| {
                            persisted.download.state = MediaDownloadState::Completed.into();
                        })
                        .await?;
                }
                Err(_) => {
                    self.db
                        .update_media_status(&id, move |persisted| {
                            persisted.download.state =
                                MediaDownloadState::Downloading { progress: 0.0,
                                                                  retry:    download.retry, }.into();
                        })
                        .await?;

                    download.retry += 1;
                    download.task = self.new_download_task(id.clone(), download.spec.clone(), self.local_path(&id))
                                        .into();

                    warn!(%id, retry = download.retry, "download failed, retrying");

                    self.downloads.insert(id.clone(), download);
                }
            }

            let _ = self.send_updates(&id).await;
        }

        Ok(())
    }

    async fn uploading_finished(&mut self, id: AppMediaObjectId, res: anyhow::Result<()>) -> anyhow::Result<()> {
        debug!(%id, ?res, "uploading finished");

        if let Some(mut upload) = self.uploads.remove(&id) {
            match res {
                Ok(_) => {
                    let relative_path = Self::relative_path(PathBuf::new(), &id);
                    self.db
                        .update_media_status(&id, |persisted| {
                            persisted.upload.state = MediaUploadState::Completed.into();
                            persisted.path = Some(relative_path);
                        })
                        .await?;
                }
                Err(_) => {
                    self.db
                        .update_media_status(&id, move |persisted| {
                            persisted.upload.state = MediaUploadState::Uploading { retry:    upload.retry,
                                                                                   progress: 0.0, }.into();
                        })
                        .await?;
                    let _ = self.send_updates(&id).await;

                    upload.retry += 1;
                    upload.task = self.new_upload_task(id.clone(), upload.spec.clone(), self.local_path(&id))
                                      .into();

                    warn!(%id, retry = upload.retry, "upload failed, retrying");

                    self.uploads.insert(id.clone(), upload);
                }
            }

            let _ = self.send_updates(&id).await;
        }

        Ok(())
    }

    async fn send_updates(&self, id: &AppMediaObjectId) -> anyhow::Result<()> {
        for session in self.db.get_sessions_for_media(id).await? {
            let media_status = self.db.get_media_status_multiple(&session.media_objects).await?;
            let media_status = media_status.into_iter()
                                           .map(|status| {
                                               (status.id.clone(),
                                                MediaObject { id:       status.id.clone(),
                                                              metadata: status.metadata,
                                                              path:     status.path,
                                                              download: status.download.state,
                                                              upload:   status.upload.state, })
                                           })
                                           .collect();

            let event = MediaServiceEvent::SessionMediaState { session_id: session.id.clone(),
                                                               media:      media_status, };

            emit_nats_event(event).await?;
        }

        Ok(())
    }

    pub async fn clean_stale_sessions(&mut self) -> anyhow::Result<()> {
        Ok(self.db.clean_stale_sessions().await?)
    }

    pub async fn restart_pending_uploads(&mut self) -> anyhow::Result<()> {
        for media in self.db.pending_uploads().await? {
            if let Some(upload) = media.upload.spec {
                self.add_upload(media.id.clone(), upload);
            }
        }

        Ok(())
    }

    pub async fn restart_pending_downloads(&mut self) -> anyhow::Result<()> {
        for media in self.db.pending_downloads().await? {
            if let Some(download) = media.download.spec {
                self.add_download(media.id.clone(), download);
            }
        }

        Ok(())
    }
}

pub fn get_media_service() -> Arc<Mutex<Service>> {
    MEDIA_SERVICE.get().expect("media service not initialized").clone()
}

pub fn init(db: Db, cfg: &Config) {
    let (_, service) = Service::new(db, cfg);
    MEDIA_SERVICE.set(service).expect("media service already initialized");
}
