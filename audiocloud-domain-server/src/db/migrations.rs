use crate::db::Db;

impl Db {
    fn migration_0000_init(&self) -> anyhow::Result<()> {
    }

    pub(crate) fn apply_migrations(&self) -> anyhow::Result<()> {
        self.migration_0000_init()?;

        Ok(())
    }
}
