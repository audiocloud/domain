use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use actix::{Actor, Addr, AsyncContext, Context, ContextFutureSpawner, Handler, Supervised, Supervisor, WrapFuture};
use anyhow::anyhow;
use clap::Args;
use once_cell::sync::OnceCell;

use audiocloud_api::newtypes::AppMediaObjectId;
use download::Downloader;
use messages::{QueueDownload, QueueUpload};
use upload::Uploader;

use crate::data::get_boot_cfg;
use crate::service::media::messages::{MediaJobState, NotifyDownloadProgress, NotifyUploadProgress};

pub mod download;
pub mod messages;
pub mod upload;

static MEDIA_SUPERVISOR: OnceCell<Addr<MediaSupervisor>> = OnceCell::new();

pub struct MediaSupervisor {
    uploads:    HashMap<AppMediaObjectId, Addr<Uploader>>,
    downloads:  HashMap<AppMediaObjectId, Addr<Downloader>>,
    media_root: PathBuf,
    client:     reqwest::Client,
}

impl MediaSupervisor {
    pub fn new(cfg: MediaOpts) -> anyhow::Result<MediaSupervisor> {
        let MediaOpts { media_root } = cfg;

        if !media_root.exists() {
            return Err(anyhow!("media root {media_root:?} does not exist"));
        }

        Ok(MediaSupervisor { uploads: HashMap::new(),
                             downloads: HashMap::new(),
                             client: reqwest::Client::new(),
                             media_root })
    }
}

#[derive(Args)]
pub struct MediaOpts {
    #[clap(long, env, default_value = "media")]
    pub media_root: PathBuf,
}

pub fn init(cfg: MediaOpts) -> anyhow::Result<Addr<MediaSupervisor>> {
    let service = MediaSupervisor::new(cfg)?;

    Ok(MEDIA_SUPERVISOR.get_or_init(move || service.start()).clone())
}

impl MediaSupervisor {
    fn update(&mut self, ctx: &mut Context<Self>) {
        self.uploads.retain(|_, uploader| uploader.connected());
        self.downloads.retain(|_, downloader| downloader.connected());
    }
}

impl Handler<NotifyDownloadProgress> for MediaSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyDownloadProgress, ctx: &mut Self::Context) -> Self::Result {
        match msg.state {
            MediaJobState::Started => {}
            MediaJobState::Retrying { .. } => {}
            MediaJobState::Finished { .. } => {
                self.downloads.remove(&msg.media_id);
            }
        }
    }
}

impl Handler<NotifyUploadProgress> for MediaSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyUploadProgress, ctx: &mut Self::Context) -> Self::Result {
        match msg.state {
            MediaJobState::Started => {}
            MediaJobState::Retrying { .. } => {}
            MediaJobState::Finished { .. } => {
                self.uploads.remove(&msg.media_id);
            }
        }
    }
}

impl Handler<QueueDownload> for MediaSupervisor {
    type Result = anyhow::Result<()>;

    fn handle(&mut self, msg: QueueDownload, ctx: &mut Context<Self>) -> Self::Result {
        let QueueDownload { media_id, download } = msg;

        let path = self.media_root
                       .join(media_id.app_id.to_string())
                       .join(media_id.media_id.to_string());

        if !path.exists() {
            return Err(anyhow!("Media file not found, cannot download"));
        }

        if let Some(actor) = self.uploads.get(&media_id) {
            if actor.connected() {
                return Err(anyhow!("Upload in progress, can't queue download until completed"));
            }
        }

        let downloader = Downloader::new(ctx.address(), self.client.clone(), path, media_id.clone(), download)?;

        // XXX: the previous downloader will be dropped and its futures will be canceled
        // XXX: if this does not work (i.e. it waits for all future to complete instead),
        // XXX: we can try to use ctx.cancel inside the actor instead
        self.downloads.insert(media_id, Supervisor::start(move |_| downloader));

        Ok(())
    }
}

impl Handler<QueueUpload> for MediaSupervisor {
    type Result = anyhow::Result<()>;

    fn handle(&mut self, msg: QueueUpload, ctx: &mut Context<Self>) -> Self::Result {
        let QueueUpload { media_id, upload } = msg;

        let path = self.media_root
                       .join(media_id.app_id.to_string())
                       .join(media_id.media_id.to_string());

        let uploader = Uploader::new(ctx.address(), self.client.clone(), path, media_id.clone(), upload)?;

        self.uploads.insert(media_id, Supervisor::start(move |_| uploader));

        Ok(())
    }
}

impl Actor for MediaSupervisor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.restarting(ctx);

        let boot = get_boot_cfg();

        const INITIAL_DELAY: Duration = Duration::from_millis(10);
        const STAGGER_DELAY: Duration = Duration::from_millis(125);

        let mut duration = INITIAL_DELAY;

        // TODO: query database for pending uploads and downloads
    }
}

impl Supervised for MediaSupervisor {
    fn restarting(&mut self, ctx: &mut <Self as Actor>::Context) {
        ctx.run_interval(Duration::from_millis(100), Self::update);
    }
}
