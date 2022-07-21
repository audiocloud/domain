use std::path::PathBuf;
use std::{env, fs};

use clap::Parser;

use audiocloud_driver::nats::NatsOpts;
use audiocloud_driver::{http_client, ConfigFile};

#[derive(Parser, Debug, Clone)]
struct DriverOpts {
    #[clap(flatten)]
    nats: NatsOpts,

    // Configuration file (array of instances)
    config_file: PathBuf,
}

#[actix::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "info,audiocloud_api=debug,audiocloud_driver=debug");
    }

    let opts = DriverOpts::parse();

    http_client::init()?;

    let cfg = serde_yaml::from_reader::<_, ConfigFile>(fs::File::open(opts.config_file)?)?;

    Ok(())
}
