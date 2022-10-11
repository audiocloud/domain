use audiocloud_api::cloud::domains::DomainConfig;
use clap::{Args, ValueEnum};
pub use messages::*;
use reqwest::Url;
use std::path::PathBuf;

mod cloud;
mod file;
mod messages;

#[derive(Args)]
pub struct ConfigOpts {
    /// Source of the config
    #[clap(short, long, env, default_value = "file", value_enum)]
    pub config_source: ConfigSource,

    /// Path to the config file
    #[clap(long, env, default_value = "config.yaml", required_if_eq("config_source", "file"))]
    pub config_file: PathBuf,

    /// The base cloud URL to use for config retrieval
    #[clap(long,
           env,
           default_value = "https://api.audiocloud.io",
           required_if_eq("config_source", "cloud"))]
    pub cloud_url: Url,

    #[clap(long, env, required_if_eq("config_source", "cloud"))]
    pub api_key: Option<String>,

    #[clap(long, env, default_value = "3600")]
    pub config_refresh_seconds: usize,
}

impl ConfigOpts {
    pub fn describe(&self) -> String {
        match self.config_source {
            ConfigSource::File => format!("file:{}", self.config_file.display()),
            ConfigSource::Cloud => format!("cloud:{}", self.cloud_url),
        }
    }
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
        ConfigSource::Cloud => Ok(cloud::get_config(cfg.cloud_url,
                                                    cfg.api_key
                                                       .expect("API key must be configured for cloud configuration"),
                                                    cfg.config_refresh_seconds).await?),
        ConfigSource::File => Ok(file::get_config(cfg.config_file).await?),
    }
}
