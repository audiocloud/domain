use clap::Parser;

use crate::db::DbConfig;

pub mod db;
pub mod rest_api;
pub mod service;

#[derive(Debug, Parser)]
pub struct Config {
    /// Port to listen on
    #[clap(short, long, env, default_value = "7400")]
    pub port: u16,

    #[clap(long, env, default_value = "0.0.0.0")]
    pub bind: String,

    #[cfg(not(windows))]
    #[clap(long, env, default_value = "/var/media_root")]
    pub root_dir: String,

    #[cfg(windows)]
    #[clap(long, env, default_value = "D:\\")]
    pub root_dir: String,

    #[clap(flatten)]
    pub db: DbConfig,
}
