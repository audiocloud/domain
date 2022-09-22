use anyhow::anyhow;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use clap::Args;
use kv::{Bucket, Config, Msgpack, Store, TransactionError};
use serde::{Deserialize, Serialize};
use tracing::debug;

use audiocloud_api::domain::DomainError;
use audiocloud_api::newtypes::AppTaskId;
use audiocloud_api::{AppMediaObjectId, MediaObject, TaskSecurity, TaskSpec};

pub mod instance;
pub mod migrations;
pub mod session;

#[derive(Clone)]
pub struct Db {
    store:            Store,
    task_info:        Bucket<'static, String, Msgpack<TaskInfo>>,
    task_specs:       Bucket<'static, String, Msgpack<TaskSpec>>,
    task_permissions: Bucket<'static, String, Msgpack<TaskSecurity>>,
    media_info:       Bucket<'static, String, Msgpack<MediaObject>>,
}

#[derive(Serialize, Deserialize)]
struct TaskInfo {}

#[derive(Serialize, Deserialize)]
struct MediaInfo {}

#[derive(Args)]
pub struct DataOpts {
    /// Sqlite database file where data for media and session cache will be stored. Use :memory: for an in-memory store
    #[clap(long, env, default_value = "database.db")]
    pub database_file: PathBuf,
}

pub async fn init(cfg: DataOpts) -> anyhow::Result<Db> {
    let url = cfg.database_file;
    debug!(%url, "Initializing database");

    let db = Store::new(Config::new(url))?;
    let db = Db { store:            db.clone(),
                  task_info:        db.bucket(Some("task_info"))?,
                  task_specs:       db.bucket(Some("task_specs"))?,
                  task_permissions: db.bucket(Some("task_permissions"))?,
                  media_info:       db.bucket(Some("media_info"))?, };

    debug!("Running migrations");

    db.apply_migrations()?;

    debug!("Migrations done");

    Ok(db)
}

impl Db {
    pub fn update_task_spec(&self,
                            id: &AppTaskId,
                            f: impl Fn(TaskSpec) -> anyhow::Result<TaskSpec>)
                            -> anyhow::Result<()> {
        self.task_specs.transaction(move |bucket| {
                            let key = id.to_string();
                            let spec = bucket.get(&key)?
                                             .ok_or_else(|| DomainError::TaskNotFound { task_id: id.clone() })?
                                             .0;

                            let spec = f(spec)?;
                            bucket.set(&key, &Msgpack(spec))?;
                            Ok(())
                        })?;

        Ok(())
    }
}

impl Db {
    pub fn get_media_for_task(&self, task_id: &AppTaskId) -> anyhow::Result<HashMap<AppMediaObjectId, MediaObject>> {
        Ok(self.task_specs
               .transaction2(&self.media_info, move |task_specs, media_info| {
                   let mut rv = HashMap::new();
                   let app_id = task_id.app_id.clone();
                   let task_spec = task_specs.get(&task_id.to_string())?
                                             .ok_or_else(|| TransactionError::Abort(anyhow!("Task not found")))?
                                             .0;

                   for media_id in task_spec.get_media_object_ids(&app_id) {
                       if let Some(media) = media_info.get(&media_id.to_string())? {
                           rv.insert(media_id, media.0);
                       }
                   }

                   Ok(rv)
               })?)
    }

    pub fn save_media(&self, media: MediaObject) -> anyhow::Result<()> {
        self.media_info.set(&media.id.to_string(), &Msgpack(media))?;

        Ok(())
    }

    pub fn create_default_media_if_not_exists(&self, id: &AppMediaObjectId) -> anyhow::Result<()> {
        self.media_info.transaction(move |media_info| {
                            if media_info.get(&id.to_string())?.is_none() {
                                media_info.set(&id.to_string(), &Msgpack(MediaObject::new(id)))?;
                            }

                            Ok(())
                        })?;

        Ok(())
    }

    pub fn set_task_info(&self, id: &AppTaskId, spec: TaskSpec) -> anyhow::Result<()> {
        self.task_specs.set(&id.to_string(), &Msgpack(spec))?;

        Ok(())
    }

    pub fn get_media_info(&self,
                          media_ids: &HashSet<AppMediaObjectId>)
                          -> anyhow::Result<HashMap<AppMediaObjectId, MediaObject>> {
        Ok(self.media_info.transaction(move |media_info| {
                               let mut rv = HashMap::new();

                               for id in media_ids {
                                   if let Some(media) = media_info.get(&id.to_string())? {
                                       rv.insert(id.clone(), media.0);
                                   }
                               }

                               Ok(rv)
                           })?)
    }
}
