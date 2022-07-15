use anyhow::anyhow;
use clap::Args;
use once_cell::sync::OnceCell;
use sqlx::SqlitePool;

#[derive(Args)]
pub struct DataOpts {
    #[clap(short, long, env, default_value = "sqlite://:memory:")]
    database_url: String,
}

static POOL: OnceCell<SqlitePool> = OnceCell::new();

pub async fn init_data(opts: DataOpts) -> anyhow::Result<()> {
    POOL.set(SqlitePool::connect(&opts.database_url).await?)
        .map_err(|_| anyhow!("Pool can be initialized only once"))?;

    Ok(())
}
