use std::path::PathBuf;

use audiocloud_api::cloud::domains::DomainConfig;

pub async fn get_config(path: PathBuf) -> anyhow::Result<DomainConfig> {
    Ok(serde_yaml::from_slice(std::fs::read(path)?.as_slice())?)
}
