use std::path::PathBuf;

use clap::Args;
use kv::{Bucket, Config, Json, Msgpack, Store};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::debug;

use audiocloud_api::{MediaObject, Model, TaskSecurity, TaskSpec};

mod instance;
mod media;
mod migrations;
mod models;
mod tasks;

#[derive(Clone)]
pub struct Db {
    store:            Store,
    system:           Bucket<'static, String, Json<Value>>,
    task_info:        Bucket<'static, String, Msgpack<TaskInfo>>,
    task_specs:       Bucket<'static, String, Msgpack<TaskSpec>>,
    task_permissions: Bucket<'static, String, Msgpack<TaskSecurity>>,
    media_info:       Bucket<'static, String, Msgpack<MediaObject>>,
    models:           Bucket<'static, String, Msgpack<Model>>,
}

#[derive(Clone, Serialize, Deserialize)]
struct TaskInfo {}

#[derive(Args)]
pub struct DataOpts {
    /// Sqlite database file where data for media and session cache will be stored. Use :memory: for an in-memory store
    #[clap(long, env, default_value = "database.db")]
    pub database_file: PathBuf,
}

pub async fn init(cfg: DataOpts) -> anyhow::Result<Db> {
    let database_file = &cfg.database_file;
    debug!(?database_file, "Initializing database");

    let db = Store::new(Config::new(database_file))?;
    let db = Db { store:            db.clone(),
                  system:           db.bucket(None)?,
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
    fn get_sys_value<T>(&self, key: &str) -> anyhow::Result<Option<T>>
        where T: DeserializeOwned
    {
        let val = self.system.get(&key.to_owned())?;

        Ok(match val {
            None => None,
            Some(value) => serde_json::from_value(value.0)?,
        })
    }

    fn set_sys_value<T>(&self, key: &str, value: T) -> anyhow::Result<()>
        where T: Serialize
    {
        let value = serde_json::to_value(&value)?;
        self.system.set(&key.to_owned(), &Json(value))?;

        Ok(())
    }
}
