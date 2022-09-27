use crate::db::Db;

impl Db {
    fn migration_0000_init(&self) -> anyhow::Result<()> {
        Ok(())
    }

    pub(crate) fn apply_migrations(&self) -> anyhow::Result<()> {
        let mut version = self.get_sys_value::<u32>("migration_version")?.unwrap_or(0);

        if version == 0 {
            self.migration_0000_init()?;
            version += 1;
        }

        self.set_sys_value("migration_version", version)?;

        Ok(())
    }
}
