use crate::data::instance::{InstancePlay, InstancePower};
use anyhow::anyhow;
use audiocloud_api::change::SessionState;
use audiocloud_api::cloud::domains::BootDomain;
use clap::Args;
use mongodb::bson::doc;
use mongodb::{Client, Database};
use once_cell::sync::OnceCell;

mod instance;
mod session;

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
    async fn new(opts: DataOpts, boot: BootDomain) -> anyhow::Result<Self> {
        let client = Client::with_uri_str(&opts.database_url).await?;
        let database = client.database(boot.domain_id.as_str());

        let sessions = database.collection::<session::Session>("sessions");
        let instances = database.collection::<instance::Instance>("instances");

        sessions.delete_many(doc! {}, None).await?;
        instances.delete_many(doc! {}, None).await?;

        for (id, session) in boot.sessions {
            sessions.insert_one(session::Session { _id:    id,
                                                   spec:   session.spec,
                                                   state:  SessionState::default(),
                                                   reaper: Default::default(), },
                                None)
                    .await?;
        }

        for (id, instance) in boot.fixed_instances {
            instances.insert_one(instance::Instance { _id:   id,
                                                      play:  instance.media.map(InstancePlay::from),
                                                      power: instance.power.map(InstancePower::from), },
                                 None)
                     .await?;
        }

        Ok(MongoDb { client, database })
    }
}

pub async fn init(opts: DataOpts, boot: BootDomain) -> anyhow::Result<()> {
    let mongo = MongoDb::new(opts, boot).await?;
    MONGO.set(mongo).map_err(|_| anyhow!("MongoDB init already called!"))?;

    Ok(())
}
