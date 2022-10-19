use std::time::Duration;

use actix::{Actor, ActorFutureExt, Addr, AsyncContext, Context, ContextFutureSpawner, WrapFuture};
use actix_broker::BrokerIssue;
use anyhow::anyhow;
use once_cell::sync::OnceCell;
use reqwest::{Client, Url};
use tracing::*;

use audiocloud_api::cloud::domains::DomainConfig;

use crate::config::NotifyDomainConfiguration;

#[instrument(skip_all, err)]
pub async fn get_config(url: Url, api_key: String) -> anyhow::Result<DomainConfig> {
    let client = Client::new();
    let url = url.join("/v1/domains/config")?;

    Ok(client.get(url)
             .bearer_auth(api_key)
             .send()
             .await?
             .json::<DomainConfig>()
             .await?)
}
