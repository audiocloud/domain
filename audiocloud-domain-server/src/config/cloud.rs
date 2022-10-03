use reqwest::Url;

use audiocloud_api::cloud::domains::DomainConfig;

pub async fn get_config(url: Url, api_key: String) -> anyhow::Result<DomainConfig> {
    let client = reqwest::Client::new();
    let url = url.join("/v1/domains/config")?;

    Ok(client.get(url)
             .bearer_auth(api_key)
             .send()
             .await?
             .json::<DomainConfig>()
             .await?)
}
