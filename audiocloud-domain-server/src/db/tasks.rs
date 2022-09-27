use kv::Msgpack;

use audiocloud_api::domain::DomainError;
use audiocloud_api::{AppTaskId, TaskSpec};

use crate::db::Db;

impl Db {
    pub fn set_task_info(&self, id: &AppTaskId, spec: TaskSpec) -> anyhow::Result<()> {
        self.task_specs.set(&id.to_string(), &Msgpack(spec))?;

        Ok(())
    }

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
