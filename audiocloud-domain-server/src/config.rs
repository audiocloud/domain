use std::path::PathBuf;

use audiocloud_api::cloud::domains::DomainConfig;
use clap::{Args, ValueEnum};
use reqwest::Url;

#[derive(Args)]
pub struct ConfigOpts {
    /// Source of the config
    #[clap(short, long, env, default_value = "file")]
    config_source: ConfigSource,

    /// Path to the config file
    #[clap(long, env, default_value_t = "config.toml", required_if_eq("config_source", "file"))]
    config_path: Option<PathBuf>,

    /// The base cloud URL to use for config retrieval
    #[clap(long,
           env,
           default_value_t = "https://api.audiocloud.io",
           required_if_eq("config_source", "cloud"))]
    cloud_url: Option<Url>,

    #[clap(long, env, required_if_eq("config_source", "cloud"))]
    api_key: Option<String>,
}

#[derive(ValueEnum)]
pub enum ConfigSource {
    /// Load the config from an cloud or orchestrator
    Cloud,
    /// Load the config from a local file
    File,
}

pub async fn init(cfg: ConfigOpts) -> anyhow::Result<DomainConfig> {
    match cfg.config_source {
        ConfigSource::Cloud => {
            let url = cfg.cloud_url.unwrap();
            let api_key = cfg.api_key.unwrap();

            Ok(cloud::get_config(url, api_key).await?)
        }
        ConfigSource::File => {
            let path = cfg.config_path.unwrap();
            Ok(file::get_config(path).await?)
        }
    }
}

mod cloud {
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
}

mod file {
    use std::path::PathBuf;

    use audiocloud_api::cloud::domains::DomainConfig;

    pub async fn get_config(path: PathBuf) -> anyhow::Result<DomainConfig> {
        Ok(toml::from_slice(std::fs::read(path)?.as_slice())?)
    }
}
