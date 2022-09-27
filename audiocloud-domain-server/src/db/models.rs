use crate::db::Db;
use audiocloud_api::{Model, ModelId};
use kv::Msgpack;

impl Db {
    pub fn delete_all_models(&self) -> anyhow::Result<()> {
        self.models.clear()?;

        Ok(())
    }

    pub fn set_model(&self, model_id: ModelId, model: Model) -> anyhow::Result<()> {
        let model_id = model_id.to_string();
        let model = Msgpack(model);

        self.models.set(&model_id, model)?;

        Ok(())
    }

    pub fn get_model(&self, model_id: &ModelId) -> anyhow::Result<Option<Model>> {
        let model_id = model_id.to_string();
        let model = self.models.get(&model_id)?;

        Ok(model.map(|model| model.0))
    }
}
