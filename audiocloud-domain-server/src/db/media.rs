use std::collections::{HashMap, HashSet};

use anyhow::anyhow;
use kv::{Msgpack, TransactionError};

use audiocloud_api::{AppMediaObjectId, AppTaskId, MediaObject};

use crate::db::Db;

impl Db {
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
}
