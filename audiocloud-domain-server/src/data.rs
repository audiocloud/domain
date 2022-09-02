use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::time::Duration;

use anyhow::anyhow;
use clap::Args;
use once_cell::sync::OnceCell;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{query, ConnectOptions, SqlitePool};
use tracing::debug;
use tracing::log::LevelFilter;

use audiocloud_api::cloud::domains::BootDomain;
use audiocloud_api::newtypes::{AppId, AppMediaObjectId, AppSessionId, MediaObjectId};

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

pub fn get_pool() -> &'static SqlitePool {
    POOL.get().expect("Database pool not initialized")
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

    sqlx::migrate!().run(&pool).await?;

    debug!("Migrations done");

    POOL.set(pool)
        .map_err(|_| anyhow!("Database pool init already called"))?;

    Ok(())
}

pub trait MediaDatabase {
    fn get_media_files_for_session<'a>(
        &'a self,
        session_id: &'a AppSessionId)
        -> Pin<Box<dyn Future<Output = anyhow::Result<HashSet<AppMediaObjectId>>> + 'a>>;

    fn set_media_files_for_session<'a>(&'a self,
                                       session_id: &'a AppSessionId,
                                       media: HashSet<AppMediaObjectId>)
                                       -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + 'a>>;
}

impl MediaDatabase for SqlitePool {
    fn get_media_files_for_session<'a>(
        &'a self,
        session_id: &'a AppSessionId)
        -> Pin<Box<dyn Future<Output = anyhow::Result<HashSet<AppMediaObjectId>>> + 'a>> {
        Box::pin(async move {
            let app_id = session_id.app_id.as_str();
            let session_id = session_id.session_id.as_str();

            let rows = query!("SELECT media_id FROM session_media WHERE app_id = ? AND session_id = ?",
                              app_id,
                              session_id).fetch_all(self)
                                         .await?;

            Ok::<_, anyhow::Error>(rows.into_iter()
                                       .map(|row| {
                                           MediaObjectId::new(row.media_id).for_app(AppId::new(app_id.to_owned()))
                                       })
                                       .collect())
        })
    }

    fn set_media_files_for_session<'a>(&'a self,
                                       session_id: &'a AppSessionId,
                                       media: HashSet<AppMediaObjectId>)
                                       -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + 'a>> {
        Box::pin(async move {
            let mut txn = self.begin().await?;

            let app_id = session_id.app_id.as_str();
            let session_id = session_id.session_id.as_str();

            query!("DELETE FROM session_media WHERE app_id = ? AND session_id = ?",
                   app_id,
                   session_id).execute(&mut txn)
                              .await?;

            for media_id in media {
                let media_id = media_id.media_id.as_str();

                query!("INSERT INTO session_media (app_id, session_id, media_id) VALUES (?, ?, ?)",
                       app_id,
                       session_id,
                       media_id).execute(&mut txn)
                                .await?;
            }

            txn.commit().await?;

            Ok::<_, anyhow::Error>(())
        })
    }
}
