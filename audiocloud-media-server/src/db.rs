use clap::Args;
use futures::TryStreamExt;
use mongodb::bson::doc;
use mongodb::{Client, Collection};
use serde::{Deserialize, Serialize};
use tracing::*;

use audiocloud_api::media::{DownloadFromDomain, MediaMetadata, UploadToDomain};
use audiocloud_api::newtypes::AppMediaObjectId;

#[derive(Serialize, Deserialize, Debug)]
pub struct PersistedMediaObject {
    pub _id:      AppMediaObjectId,
    pub metadata: Option<MediaMetadata>,
    pub path:     Option<String>,
    pub download: Option<DownloadFromDomain>,
    pub upload:   Option<UploadToDomain>,
}

impl PersistedMediaObject {
    pub fn new(_id: AppMediaObjectId) -> Self {
        Self { _id,
               metadata: None,
               upload: None,
               download: None,
               path: None }
    }
}

#[derive(Clone)]
pub struct Db {
    mongo: Client,
    files: Collection<PersistedMediaObject>,
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
        let files = mongo.default_database().expect("default database").collection("files");

        Ok(Self { mongo, files })
    }

    #[instrument(skip(self), err)]
    pub async fn get_media_status(&self, id: &AppMediaObjectId) -> anyhow::Result<Option<PersistedMediaObject>> {
        Ok(self.files.find_one(doc! {"_id": id.to_string()}, None).await?)
    }

    #[instrument(skip(self, ids), err)]
    pub async fn get_media_status_multiple(&self,
                                           ids: impl Iterator<Item = &AppMediaObjectId>)
                                           -> anyhow::Result<Vec<PersistedMediaObject>> {
        let ids = ids.map(|id| id.to_string()).collect::<Vec<_>>();

        Ok(self.files
               .find(doc! {"_id": {"$in": ids}}, None)
               .await?
               .try_collect::<Vec<_>>()
               .await?)
    }

    #[instrument(skip(self), err)]
    pub async fn set_media_status(&self, id: &AppMediaObjectId, state: PersistedMediaObject) -> anyhow::Result<()> {
        self.files.insert_one(state, None).await?;
        Ok(())
    }

    #[instrument(skip(self, func), err)]
    pub async fn update_media_status(&self,
                                     id: &AppMediaObjectId,
                                     func: impl FnOnce(&mut PersistedMediaObject) -> () + Send + Sync + 'static)
                                     -> anyhow::Result<PersistedMediaObject> {
        let mut txn = self.mongo.start_session(None).await?;
        txn.start_transaction(None).await?;

        let read = self.files
                       .find_one_with_session(doc! {"_id": id.to_string()}, None, &mut txn)
                       .await?;

        let mut read = match read {
            None => PersistedMediaObject::new(id.clone()),
            Some(read) => read,
        };

        func(&mut read);
        self.files.insert_one_with_session(&read, None, &mut txn).await?;

        txn.commit_transaction().await?;

        Ok(read)
    }
}
