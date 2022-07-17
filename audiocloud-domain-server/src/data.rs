use anyhow::anyhow;
use clap::Args;
use mongodb::{Client, Database};
use once_cell::sync::OnceCell;

use audiocloud_api::newtypes::DomainId;

#[derive(Args)]
pub struct DataOpts {
    #[clap(short, long, env, default_value = "mongodb://localhost:27017")]
    database_url: String,
}

static MONGO: OnceCell<MongoDb> = OnceCell::new();

pub struct MongoDb {
    pub client:   Client,
    pub database: Database,
}

impl MongoDb {
    async fn new(opts: DataOpts, domain: &DomainId) -> anyhow::Result<Self> {
        let client = Client::with_uri_str(&opts.database_url).await?;
        let database = client.database(domain.as_str());

        Ok(MongoDb { client, database })
    }
}

pub async fn init(opts: DataOpts, domain: &DomainId) -> anyhow::Result<()> {
    let mongo = MongoDb::new(opts, domain).await?;
    MONGO.set(mongo).map_err(|_| anyhow!("MongoDB init already called!"))?;

    Ok(())
}
