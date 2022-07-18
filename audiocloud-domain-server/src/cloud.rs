use anyhow::anyhow;
use clap::Args;
use once_cell::sync::OnceCell;
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::Url;

use audiocloud_api::cloud::domains::BootDomain;
use audiocloud_api::newtypes::DomainId;

#[derive(Args, Debug)]
pub struct CloudOpts {
    #[clap(short, long, env)]
    api_key: String,

    #[clap(short, long, env, default_value = "https://api.audiocloud.org")]
    api_url: Url,
}

struct CloudClient {
    client: reqwest::Client,
    boot:   BootDomain,
}

static CLOUD_CLIENT: OnceCell<CloudClient> = OnceCell::new();

pub async fn init(opts: CloudOpts) -> anyhow::Result<DomainId> {
    let mut headers = HeaderMap::new();
    headers.insert("X-Api-Key", HeaderValue::from_str(&opts.api_key)?);

    let client = reqwest::Client::builder().default_headers(headers)
                                           .tcp_nodelay(true)
                                           .build()?;

    let boot = client.get(opts.api_url.join("/v1/domains/boot")?)
                     .send()
                     .await?
                     .json::<BootDomain>()
                     .await?;

    let domain_id = boot.domain_id.clone();

    CLOUD_CLIENT.set(CloudClient { client, boot })
                .map_err(|_| anyhow!("Cloud client must only be called once"))?;

    Ok(domain_id)
}
