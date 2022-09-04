use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::time::Duration;

use anyhow::anyhow;
use clap::Args;
use once_cell::sync::OnceCell;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::testing::TestTermination;
use sqlx::{query, ConnectOptions, Executor, Sqlite, SqlitePool};
use tracing::debug;
use tracing::log::LevelFilter;

use audiocloud_api::cloud::domains::BootDomain;
use audiocloud_api::media::{DownloadFromDomain, MediaDownload, MediaObject, MediaUpload, UploadToDomain};
use audiocloud_api::newtypes::{AppId, AppMediaObjectId, AppSessionId, MediaObjectId};

pub mod instance;
pub mod reaper;
pub mod session;

#[derive(Args)]
pub struct DataOpts {
    #[clap(long, env, default_value = "sqlite::memory:")]
    pub database_url: String,
}

static BOOT_CFG: OnceCell<BootDomain> = OnceCell::new();
static SQLITE_POOL: OnceCell<SqlitePool> = OnceCell::new();

pub fn get_boot_cfg() -> &'static BootDomain {
    BOOT_CFG.get().expect("Boot state not initialized")
}

pub fn get_sqlite_pool() -> &'static SqlitePool {
    SQLITE_POOL.get().expect("Database pool not initialized")
}

pub async fn init(cfg: DataOpts, boot: BootDomain) -> anyhow::Result<()> {
    BOOT_CFG.set(boot).map_err(|_| anyhow!("State init already called!"))?;

    debug!(url = &cfg.database_url, "Initializing database");

    let mut opts = SqliteConnectOptions::from_str(&cfg.database_url)?.create_if_missing(true)
                                                                     .journal_mode(SqliteJournalMode::Wal)
                                                                     .synchronous(SqliteSynchronous::Normal);

    opts.log_statements(LevelFilter::Off)
        .log_slow_statements(LevelFilter::Debug, Duration::from_millis(125));

    let pool = SqlitePool::connect_with(opts).await?;

    debug!("Running migrations");

    sqlx::migrate!().run(&pool).await?;

    debug!("Migrations done");

    SQLITE_POOL.set(pool)
               .map_err(|_| anyhow!("Database pool init already called"))?;

    Ok(())
}

#[derive(Debug, Clone)]
pub struct MediaDatabase {
    pool: SqlitePool,
}

impl Default for MediaDatabase {
    fn default() -> Self {
        MediaDatabase { pool: get_sqlite_pool().clone(), }
    }
}

impl MediaDatabase {
    pub async fn get_media_files_for_session(&self,
                                             session_id: &AppSessionId)
                                             -> anyhow::Result<HashSet<AppMediaObjectId>> {
        let app_id = session_id.app_id.as_str();
        let session_id = session_id.session_id.as_str();

        let rows = query!("SELECT media_id FROM session_media WHERE app_id = ? AND session_id = ?",
                          app_id,
                          session_id).fetch_all(&self.pool)
                                     .await?;

        Ok::<_, anyhow::Error>(rows.into_iter()
                                   .map(|row| MediaObjectId::new(row.media_id).for_app(AppId::new(app_id.to_owned())))
                                   .collect())
    }

    pub async fn set_media_files_for_session(&self,
                                             session_id: &AppSessionId,
                                             media: HashSet<AppMediaObjectId>)
                                             -> anyhow::Result<()> {
        let mut txn = self.pool.begin().await?;

        let app_id = session_id.app_id.as_str();
        let session_id = session_id.session_id.as_str();

        query!("DELETE FROM session_media WHERE app_id = ? AND session_id = ?",
               app_id,
               session_id).execute(&mut txn)
                          .await?;

        for media_id in media {
            let media_id = media_id.media_id.as_str();

            query!("INSERT INTO session_media (app_id, session_id, media_id) VALUES (?, ?, ?)",
                   app_id,
                   session_id,
                   media_id).execute(&mut txn)
                            .await?;
        }

        txn.commit().await?;

        Ok::<_, anyhow::Error>(())
    }

    pub async fn get_media<'a>(&self, app_media_id: &AppMediaObjectId) -> anyhow::Result<Option<MediaObject>> {
        Self::get_media_txn(app_media_id, &self.pool).await
    }

    async fn get_media_txn<'a>(app_media_id: &AppMediaObjectId,
                               txn: impl Executor<'a, Database = Sqlite>)
                               -> anyhow::Result<Option<MediaObject>> {
        let app_id = app_media_id.app_id.as_str();
        let media_id = app_media_id.media_id.as_str();

        let media_row = query!("SELECT * FROM media WHERE app_id = ? AND media_id = ?",
                               app_id,
                               media_id).fetch_optional(txn)
                                        .await?;

        let media = match media_row {
            Some(media_row) => {
                let metadata_json = match media_row.metadata {
                    Some(metadata) => Some(serde_json::from_str(&metadata)?),
                    None => None,
                };

                let maybe_download = match media_row.download {
                    Some(download) => Some(serde_json::from_str(&download)?),
                    None => None,
                };
                let maybe_upload = match media_row.upload {
                    Some(upload) => Some(serde_json::from_str(&upload)?),
                    None => None,
                };

                Some(MediaObject { id:       app_media_id.clone(),
                                   metadata: metadata_json,
                                   path:     media_row.path,
                                   download: maybe_download,
                                   upload:   maybe_upload, })
            }
            None => None,
        };

        Ok::<_, anyhow::Error>(media)
    }

    pub async fn update_media(&self,
                              media: &AppMediaObjectId,
                              update: impl Fn(&mut MediaObject) -> anyhow::Result<()>)
                              -> anyhow::Result<()> {
        for _retry in 0..10 {
            let mut txn = self.pool.begin().await?;

            match Self::get_media_txn(media, &mut txn).await? {
                None => {
                    return Err(anyhow!("Media object not found"));
                }
                Some(mut media) => {
                    update(&mut media)?;

                    Self::save_media_txn(media, &mut txn).await?;
                }
            }

            if txn.commit().await?.is_success() {
                return Ok(());
            }
        }

        Err(anyhow!("Failed to commit transaction, too many retries"))
    }

    pub async fn save_media(&self, media: MediaObject) -> anyhow::Result<()> {
        Self::save_media_txn(media, &self.pool).await
    }

    async fn create_media_txn<'a>(media: MediaObject, txn: impl Executor<'a, Database = Sqlite>) -> anyhow::Result<()> {
        let app_id = media.id.app_id.as_str();
        let media_id = media.id.media_id.as_str();

        let metadata_json = match media.metadata {
            Some(metadata) => Some(serde_json::to_string(&metadata)?),
            None => None,
        };

        let download_in_progress = media.download.as_ref().map(|download| download.state.in_progress);
        let upload_in_progress = media.upload.as_ref().map(|upload| upload.state.in_progress);
        let media_download = serde_json::to_string(&media.download)?;
        let media_upload = serde_json::to_string(&media.upload)?;

        query!("INSERT INTO media (app_id, media_id, path, metadata, download, upload, download_in_progress, upload_in_progress) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
               app_id,
               media_id,
               media.path,
               metadata_json,
               media_download,
               media_upload,
               download_in_progress,
               upload_in_progress).execute(txn)
                            .await?;

        Ok::<_, anyhow::Error>(())
    }

    async fn save_media_txn<'a>(media: MediaObject, txn: impl Executor<'a, Database = Sqlite>) -> anyhow::Result<()> {
        let MediaObject { id,
                          metadata,
                          path,
                          download,
                          upload, } = media;

        let app_id = id.app_id.as_str();
        let media_id = id.media_id.as_str();
        let download_in_progress = download.as_ref().map(|download| download.state.in_progress);
        let upload_in_progress = upload.as_ref().map(|upload| upload.state.in_progress);
        let download = serde_json::to_string(&download)?;
        let upload = serde_json::to_string(&upload)?;
        let metadata = match metadata {
            Some(metadata) => Some(serde_json::to_string(&metadata)?),
            None => None,
        };

        query!("UPDATE media SET metadata = ?, path = ?, download = ?, upload = ?, download_in_progress = ?, upload_in_progress = ? WHERE app_id = ? AND media_id = ?",
                           metadata,
                           path,
                           download,
                           upload,
                           download_in_progress,
                           upload_in_progress,
                           app_id,
                           media_id).execute(txn).await?;

        Ok(())
    }

    pub async fn get_pending_downloads_uploads(
        &self)
        -> anyhow::Result<(HashMap<AppMediaObjectId, DownloadFromDomain>, HashMap<AppMediaObjectId, UploadToDomain>)>
    {
        let mut txn = self.pool.begin().await?;

        let rows = query!("SELECT app_id, media_id, upload, download FROM media WHERE download_in_progress = 1 OR upload_in_progress = 1").fetch_all(&mut txn).await?;

        let mut downloads = HashMap::new();
        let mut uploads = HashMap::new();

        for row in rows {
            let app_media_id = MediaObjectId::new(row.media_id).for_app(AppId::new(row.app_id));

            if let Some(download) = row.download {
                let download: MediaDownload = serde_json::from_str(&download)?;
                if download.state.in_progress {
                    downloads.insert(app_media_id.clone(), download.download);
                }
            }

            if let Some(upload) = row.upload {
                let upload: MediaUpload = serde_json::from_str(&upload)?;
                if upload.state.in_progress {
                    uploads.insert(app_media_id, upload.upload);
                }
            }
        }

        Ok((downloads, uploads))
    }
}
