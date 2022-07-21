use anyhow::anyhow;
use clap::Args;
use once_cell::sync::OnceCell;

use audiocloud_api::cloud::domains::BootDomain;

pub mod instance;
pub mod reaper;
pub mod session;

#[derive(Args)]
pub struct DataOpts {}

static BOOT: OnceCell<BootDomain> = OnceCell::new();

pub fn get_boot_cfg() -> &'static BootDomain {
    BOOT.get().expect("Boot state not initialized")
}

pub async fn init(boot: BootDomain) -> anyhow::Result<()> {
    BOOT.set(boot).map_err(|_| anyhow!("State init already called!"))?;

    Ok(())
}
