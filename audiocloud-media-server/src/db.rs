use std::collections::HashSet;

use chrono::{NaiveDateTime, Utc};
use clap::Args;
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use sqlx::{query, SqlitePool};
use tracing::*;

use audiocloud_api::media::{DownloadFromDomain, MediaDownloadState, MediaMetadata, MediaUploadState, UploadToDomain};
use audiocloud_api::newtypes::{AppMediaObjectId, AppSessionId};
use audiocloud_api::time::{Timestamp, Timestamped};

#[derive(Serialize, Deserialize, Debug)]
pub struct PersistedMediaObject {
    pub id:       AppMediaObjectId,
    pub metadata: Option<MediaMetadata>,
    pub path:     Option<String>,
    pub download: PersistedDownload,
    pub upload:   PersistedUpload,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PersistedSession {
    pub id:            AppSessionId,
    pub media_objects: HashSet<AppMediaObjectId>,
    pub ends_at:       Timestamp,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PersistedDownload {
    pub spec:  Option<DownloadFromDomain>,
    pub state: Timestamped<MediaDownloadState>,
}

impl PersistedDownload {
    pub fn new(spec: Option<DownloadFromDomain>) -> Self {
        Self { spec,
               state: MediaDownloadState::Pending.into() }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PersistedUpload {
    pub spec:  Option<UploadToDomain>,
    pub state: Timestamped<MediaUploadState>,
}

impl PersistedUpload {
    pub fn new(spec: Option<UploadToDomain>) -> Self {
        Self { spec,
               state: MediaUploadState::Pending.into() }
    }
}

impl PersistedMediaObject {
    pub fn new(id: AppMediaObjectId) -> Self {
        Self { id,
               metadata: None,
               upload: PersistedUpload::new(None),
               download: PersistedDownload::new(None),
               path: None }
    }
}

#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
}

#[derive(Debug, Args)]
pub struct DbConfig {
    #[clap(long, env, default_value = "sqlite::memory:")]
    pub database_url: String,
}

impl Db {
    #[instrument(skip_all, err)]
    pub async fn new(config: &DbConfig) -> anyhow::Result<Self> {
        let pool = SqlitePool::connect(&config.database_url).await?;

        Ok(Self { pool })
    }

    #[instrument(skip(self), err)]
    pub async fn get_media_status(&self, id: &AppMediaObjectId) -> anyhow::Result<Option<PersistedMediaObject>> {
        let id = id.to_string();
        let rv = query!("SELECT * FROM media WHERE id = ?", id).fetch_optional(&self.pool)
                                                               .await?;

        Ok(match rv {
            None => None,
            Some(model) => {
                Some(persistent_media_object(model.id, model.metadata, model.path, model.download, model.upload)?)
            }
        })
    }

    #[instrument(skip(self), err)]
    pub async fn get_session_state(&self, id: &AppSessionId) -> anyhow::Result<Option<PersistedSession>> {
        let id = id.to_string();
        let mut txn = self.pool.begin().await?;

        let rv = query!("SELECT * FROM session WHERE id = ?", id).fetch_optional(&mut txn)
                                                                 .await?;

        Ok(match rv {
            None => None,
            Some(model) => {
                let mut rv = persistent_session(model.id, model.ends_at)?;
                let session_id = rv.id.to_string();

                let mut media =
                    query!("SELECT session_media.media_id FROM session_media WHERE session_media.session_id = ?",
                           session_id).fetch(&mut txn);

                while let Some(media) = media.try_next().await? {
                    rv.media_objects.insert(media.media_id.parse()?);
                }

                drop(media);

                txn.commit().await?;

                Some(rv)
            }
        })
    }

    #[instrument(skip(self), err)]
    pub async fn set_session_state(&self, state: PersistedSession) -> anyhow::Result<&str> {
        let mut txn = self.pool.begin().await?;

        let id = state.id.to_string();
        let ends_at = state.ends_at.naive_utc().timestamp();

        // upsert session
        query!("INSERT OR REPLACE INTO session (id, ends_at) VALUES (?, ?)",
               id,
               ends_at).execute(&mut txn)
                       .await?;

        // delete existing media objects
        query!("DELETE FROM session_media WHERE session_id = ?", id).execute(&mut txn)
                                                                    .await?;

        // insert new media objects
        for media_id in state.media_objects {
            let media_id = media_id.to_string();

            query!("INSERT INTO session_media (session_id, media_id) VALUES (?, ?)",
                   id,
                   media_id).execute(&mut txn)
                            .await?;
        }

        txn.commit().await?;

        Ok("done")
    }

    #[instrument(skip(self), err)]
    pub async fn delete_session_state(&self, id: &AppSessionId) -> anyhow::Result<bool> {
        let id = id.to_string();
        let deleted = query!("DELETE FROM session WHERE id = ?", id).execute(&self.pool)
                                                                    .await?
                                                                    .rows_affected();
        Ok(deleted > 0)
    }

    #[instrument(skip(self), err)]
    pub async fn set_media_status(&self, id: &AppMediaObjectId, state: PersistedMediaObject) -> anyhow::Result<()> {
        let id = state.id.to_string();
        let metadata = match state.metadata.as_ref() {
            Some(metadata) => Some(serde_json::to_string(metadata)?),
            None => None,
        };
        let path = state.path.clone();
        let download = serde_json::to_string(&state.download)?;
        let upload = serde_json::to_string(&state.upload)?;

        query!("INSERT OR REPLACE INTO media (id, metadata, path, download, upload) VALUES (?, ?, ?, ?, ?)",
               id,
               metadata,
               path,
               download,
               upload).execute(&self.pool)
                      .await?;
        Ok(())
    }

    #[instrument(skip(self, func), err)]
    pub async fn update_media_status(&self,
                                     id: &AppMediaObjectId,
                                     func: impl FnOnce(&mut PersistedMediaObject) -> () + Send + Sync + 'static)
                                     -> anyhow::Result<PersistedMediaObject> {
        let id_str = id.to_string();

        let mut txn = self.pool.begin().await?;

        let read = query!("SELECT * FROM media WHERE id = ?", id_str).fetch_optional(&mut txn)
                                                                     .await?;

        let (mut read, exists) = match read {
            Some(read) => {
                (persistent_media_object(read.id, read.metadata, read.path, read.download, read.upload)?, true)
            }
            None => (PersistedMediaObject::new(id.clone()), false),
        };

        func(&mut read);

        let metadata = match read.metadata.as_ref() {
            Some(metadata) => Some(serde_json::to_string(metadata)?),
            None => None,
        };
        let download = serde_json::to_string(&read.download)?;
        let upload = serde_json::to_string(&read.upload)?;
        let path = read.path.clone();

        if !exists {
            query!("INSERT OR REPLACE INTO media (id, metadata, path, download, upload) VALUES (?, ?, ?, ?, ?)",
                   id_str,
                   metadata,
                   path,
                   download,
                   upload).execute(&mut txn)
                          .await?;
        } else {
            query!("UPDATE media SET metadata = ?, path = ?, download = ?, upload = ? WHERE id = ?",
                   metadata,
                   path,
                   download,
                   upload,
                   id_str).execute(&mut txn)
                          .await?;
        }

        txn.commit().await?;

        Ok(read)
    }

    #[instrument(skip_all, err)]
    pub async fn get_media_status_multiple(&self,
                                           ids: &HashSet<AppMediaObjectId>)
                                           -> anyhow::Result<Vec<PersistedMediaObject>> {
        let mut rv = Vec::new();
        for id in ids {
            if let Some(media) = self.get_media_status(id).await? {
                rv.push(media);
            }
        }

        Ok(rv)
    }

    #[instrument(skip_all, err)]
    pub async fn get_sessions_for_media(&self, id: &AppMediaObjectId) -> anyhow::Result<Vec<PersistedSession>> {
        let mut sessions = Vec::new();
        let id = id.to_string();

        let mut txn = self.pool.begin().await?;

        let mut rows = query!("SELECT session.id, session.ends_at FROM session_media
                               INNER JOIN session ON session.id = session_media.session_id
                               WHERE session_media.media_id = ?",
                              id).fetch(&mut txn);

        while let Some(row) = rows.try_next().await? {
            sessions.push(persistent_session(row.id.clone(), row.ends_at)?);
        }

        drop(rows);

        for session in &mut sessions {
            let session_id = session.id.to_string();
            let mut media =
                query!("SELECT session_media.media_id FROM session_media WHERE session_media.session_id = ?",
                       session_id).fetch(&mut txn);

            while let Some(media) = media.try_next().await? {
                session.media_objects.insert(media.media_id.parse()?);
            }
        }

        txn.commit().await?;

        Ok(sessions)
    }
}

fn persistent_media_object(id: String,
                           metadata: Option<String>,
                           path: Option<String>,
                           download: String,
                           upload: String)
                           -> anyhow::Result<PersistedMediaObject> {
    Ok(PersistedMediaObject { id: id.parse()?,
                              path,
                              download: serde_json::from_str(&download)?,
                              upload: serde_json::from_str(&upload)?,
                              metadata: match metadata {
                                  None => None,
                                  Some(metadata) => serde_json::from_str(&metadata)?,
                              } })
}

fn persistent_session(id: String, ends_at: i64) -> anyhow::Result<PersistedSession> {
    Ok(PersistedSession { id:            id.parse()?,
                          ends_at:       Timestamp::from_utc(NaiveDateTime::from_timestamp(ends_at, 0), Utc),
                          media_objects: HashSet::new(), })
}
