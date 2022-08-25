use std::collections::HashSet;

use clap::Args;
use futures::TryStreamExt;
use mongodb::bson::doc;
use mongodb::options::FindOneAndReplaceOptions;
use mongodb::{Client, Collection};
use serde::{Deserialize, Serialize};
use tracing::*;

use audiocloud_api::media::{DownloadFromDomain, MediaDownloadState, MediaMetadata, MediaUploadState, UploadToDomain};
use audiocloud_api::newtypes::{AppMediaObjectId, AppSessionId};
use audiocloud_api::time::{Timestamp, Timestamped};

#[derive(Serialize, Deserialize, Debug)]
pub struct PersistedMediaObject {
    pub _id:      AppMediaObjectId,
    pub metadata: Option<MediaMetadata>,
    pub path:     Option<String>,
    pub download: PersistedDownload,
    pub upload:   PersistedUpload,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PersistedSession {
    pub _id:           AppSessionId,
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
    pub fn new(_id: AppMediaObjectId) -> Self {
        Self { _id,
               metadata: None,
               upload: PersistedUpload::new(None),
               download: PersistedDownload::new(None),
               path: None }
    }
}

#[derive(Clone)]
pub struct Db {
    mongo:    Client,
    objects:  Collection<PersistedMediaObject>,
    sessions: Collection<PersistedSession>,
}

#[derive(Debug, Args)]
pub struct DbConfig {
    #[clap(long,
           env,
           default_value = "mongodb://127.0.0.1:27017/audiocloud_media?directConnection=true")]
    pub url: String,
}

impl Db {
    pub async fn new(config: &DbConfig) -> anyhow::Result<Self> {
        let mongo = Client::with_uri_str(&config.url).await?;
        let database = mongo.default_database().expect("default database");
        let objects = database.collection("objects");
        let sessions = database.collection("sessions");

        Ok(Self { mongo,
                  objects,
                  sessions })
    }

    #[instrument(skip(self), err)]
    pub async fn get_media_status(&self, id: &AppMediaObjectId) -> anyhow::Result<Option<PersistedMediaObject>> {
        Ok(self.objects.find_one(doc! {"_id": id.to_string()}, None).await?)
    }

    #[instrument(skip(self, ids), err)]
    pub async fn get_media_status_multiple(&self,
                                           ids: impl Iterator<Item = &AppMediaObjectId>)
                                           -> anyhow::Result<Vec<PersistedMediaObject>> {
        let ids = ids.map(|id| id.to_string()).collect::<Vec<_>>();

        Ok(self.objects
               .find(doc! {"_id": {"$in": ids}}, None)
               .await?
               .try_collect::<Vec<_>>()
               .await?)
    }

    #[instrument(skip(self), err)]
    pub async fn get_session_state(&self, id: &AppSessionId) -> anyhow::Result<Option<PersistedSession>> {
        Ok(self.sessions.find_one(doc! {"_id": id.to_string()}, None).await?)
    }

    #[instrument(skip(self), err)]
    pub async fn set_session_state(&self, state: PersistedSession) -> anyhow::Result<&str> {
        let result = self.sessions
                         .find_one_and_replace(doc! {"_id": state._id.to_string()},
                                               state,
                                               FindOneAndReplaceOptions::builder().upsert(true).build())
                         .await?;

        Ok(if result.is_some() { "updated" } else { "created" })
    }

    #[instrument(skip(self), err)]
    pub async fn delete_session_state(&self, id: &AppSessionId) -> anyhow::Result<bool> {
        let deleted = self.sessions.delete_one(doc! {"_id": id.to_string()}, None).await?;
        Ok(deleted.deleted_count > 0)
    }

    #[instrument(skip(self), err)]
    pub async fn set_media_status(&self, id: &AppMediaObjectId, state: PersistedMediaObject) -> anyhow::Result<()> {
        self.objects.insert_one(state, None).await?;
        Ok(())
    }

    #[instrument(skip(self, func), err)]
    pub async fn update_media_status(&self,
                                     id: &AppMediaObjectId,
                                     func: impl FnOnce(&mut PersistedMediaObject) -> () + Send + Sync + 'static)
                                     -> anyhow::Result<PersistedMediaObject> {
        let mut txn = self.mongo.start_session(None).await?;
        txn.start_transaction(None).await?;

        let read = self.objects
                       .find_one_with_session(doc! {"_id": id.to_string()}, None, &mut txn)
                       .await?;

        let mut read = match read {
            None => PersistedMediaObject::new(id.clone()),
            Some(read) => read,
        };

        func(&mut read);
        self.objects.insert_one_with_session(&read, None, &mut txn).await?;

        txn.commit_transaction().await?;

        Ok(read)
    }

    #[instrument(skip_all, err)]
    pub async fn get_sessions_for_media(&self, id: &AppMediaObjectId) -> anyhow::Result<Vec<PersistedSession>> {
        Ok(self.sessions
               .find(doc! {"media_objects": id.to_string()}, None)
               .await?
               .try_collect::<Vec<_>>()
               .await?)
    }
}
