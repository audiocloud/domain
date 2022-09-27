use std::path::PathBuf;

use audiocloud_api::cloud::domains::DomainConfig;
use clap::{Args, ValueEnum};
use reqwest::Url;

#[derive(Args)]
pub struct ConfigOpts {
    /// Source of the config
    #[clap(short, long, env, default_value = "file", value_enum)]
    pub config_source: ConfigSource,

    /// Path to the config file
    #[clap(long, env, default_value = "config.toml", required_if_eq("config_source", "file"))]
    pub config_file: PathBuf,

    /// The base cloud URL to use for config retrieval
    #[clap(long,
           env,
           default_value = "https://api.audiocloud.io",
           required_if_eq("config_source", "cloud"))]
    pub cloud_url: Url,

    #[clap(long, env, required_if_eq("config_source", "cloud"))]
    pub api_key: Option<String>,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum ConfigSource {
    /// Load the config from an cloud or orchestrator
    Cloud,
    /// Load the config from a local file
    File,
}

pub async fn init(cfg: ConfigOpts) -> anyhow::Result<DomainConfig> {
    match cfg.config_source {
        ConfigSource::Cloud => {
            Ok(cloud::get_config(cfg.cloud_url,
                                 cfg.api_key.expect("API key must be configured for cloud configuration")).await?)
        }
        ConfigSource::File => Ok(file::get_config(cfg.config_file).await?),
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
