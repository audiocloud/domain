use crate::DbConfig;
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
pub struct Config {
    /// Port to listen on
    #[clap(short, long, env, default_value = "7400")]
    pub port: u16,

    #[clap(long, env, default_value = "0.0.0.0")]
    pub bind: String,

    #[cfg(not(windows))]
    #[clap(long, env, default_value = "/var/media_root")]
    pub root_dir: PathBuf,

    #[cfg(windows)]
    #[clap(long, env, default_value = "D:\\")]
    pub root_dir: PathBuf,

    #[clap(flatten)]
    pub db: DbConfig,
}
