use std::str::FromStr;
use std::time::Duration;

use anyhow::anyhow;
use clap::Args;
use once_cell::sync::OnceCell;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{ConnectOptions, SqlitePool};
use tracing::debug;
use tracing::log::LevelFilter;

use audiocloud_api::cloud::domains::BootDomain;

pub mod instance;
pub mod reaper;
pub mod session;

#[derive(Args)]
pub struct DataOpts {
    #[clap(long, env, default_value = "sqlite::memory:")]
    pub database_url: String,
}

static BOOT: OnceCell<BootDomain> = OnceCell::new();
static POOL: OnceCell<SqlitePool> = OnceCell::new();

pub fn get_boot_cfg() -> &'static BootDomain {
    BOOT.get().expect("Boot state not initialized")
}

pub async fn init(cfg: DataOpts, boot: BootDomain) -> anyhow::Result<()> {
    BOOT.set(boot).map_err(|_| anyhow!("State init already called!"))?;

    debug!(url = &cfg.database_url, "Initializing database");

    let mut opts = SqliteConnectOptions::from_str(&cfg.database_url)?.create_if_missing(true)
                                                                     .journal_mode(SqliteJournalMode::Wal)
                                                                     .synchronous(SqliteSynchronous::Normal);

    opts.log_statements(LevelFilter::Off)
        .log_slow_statements(LevelFilter::Debug, Duration::from_millis(125));

    let pool = SqlitePool::connect_with(opts).await?;

    debug!("Running migrations");

    let rv = sqlx::migrate!().run(&pool).await?;

    debug!("Migrations done");

    POOL.set(pool)
        .map_err(|_| anyhow!("Database pool init already called"))?;

    Ok(())
}
