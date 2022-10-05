use std::time::Duration;

use actix::{Actor, ActorFutureExt, Addr, AsyncContext, Context, ContextFutureSpawner, WrapFuture};
use actix_broker::BrokerIssue;
use anyhow::anyhow;
use once_cell::sync::OnceCell;
use reqwest::{Client, Url};
use tracing::*;

use audiocloud_api::cloud::domains::DomainConfig;

use crate::config::NotifyDomainConfiguration;

static CLOUD_REFRESH_CONFIG: OnceCell<Addr<CloudRefreshConfig>> = OnceCell::new();

pub async fn get_config(url: Url, api_key: String, refresh_seconds: usize) -> anyhow::Result<DomainConfig> {
    let client = Client::new();
    let url = url.join("/v1/domains/config")?;

    CLOUD_REFRESH_CONFIG
      .set(CloudRefreshConfig::new(client.clone(), url.clone(), api_key.clone(), refresh_seconds).start())
      .map_err(|_| anyhow!("CLOUD_REFRESH_CONFIG already initialized"))?;

    Ok(client.get(url)
             .bearer_auth(api_key)
             .send()
             .await?
             .json::<DomainConfig>()
             .await?)
}

#[derive(Clone)]
struct CloudRefreshConfig {
    client:          Client,
    url:             Url,
    api_key:         String,
    refresh_seconds: usize,
    running:         bool,
}

impl CloudRefreshConfig {
    pub fn new(client: Client, url: Url, api_key: String, refresh_seconds: usize) -> Self {
        Self { client:          { client },
               api_key:         { api_key },
               url:             { url },
               refresh_seconds: { refresh_seconds },
               running:         { false }, }
    }

    fn get_and_update(&mut self, ctx: &mut Context<Self>) {
        if self.running {
            return;
        }

        let Self { client, url, api_key, .. } = self.clone();
        self.running = true;
        async move {
            Ok::<_, anyhow::Error>(client.get(url)
                                         .bearer_auth(api_key)
                                         .send()
                                         .await?
                                         .json::<DomainConfig>()
                                         .await?)
        }.into_actor(self)
         .map(|result, actor, _ctx| {
             actor.running = false;
             match result {
                 Ok(config) => actor.issue_system_async(NotifyDomainConfiguration { config }),
                 Err(error) => {
                     warn!(%error, "Failed to load domain config");
                 }
             }
         })
         .spawn(ctx);
    }
}

impl Actor for CloudRefreshConfig {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.run_later(Duration::from_secs(self.refresh_seconds as u64), |actor, ctx| {
               ctx.run_interval(Duration::from_secs(actor.refresh_seconds as u64), Self::get_and_update);
           });
    }
}
