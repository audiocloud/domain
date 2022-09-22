#![allow(unused_variables)]

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use actix::{Actor, ActorFutureExt, Addr, AsyncContext, Context, Handler, Supervised, Supervisor};
use actix_broker::{BrokerIssue, BrokerSubscribe};
use anyhow::anyhow;
use clap::Args;
use once_cell::sync::OnceCell;
use tracing::error;

use audiocloud_api::common::media::MediaObject;
use audiocloud_api::newtypes::AppMediaObjectId;
use download::Downloader;
use messages::{QueueDownload, QueueUpload};
use upload::Uploader;

use crate::db::Db;
use crate::media::messages::{
    DownloadJobId, ImportMedia, NotifyDownloadProgress, NotifyUploadProgress, RestartPendingUploadsDownloads,
    UploadJobId,
};
use crate::task::messages::{NotifyMediaTaskState, NotifyTaskSpec};

pub mod download;
pub mod messages;
pub mod upload;

static MEDIA_SUPERVISOR: OnceCell<Addr<MediaSupervisor>> = OnceCell::new();

struct MediaJobs {
    download: Option<DownloadJobId>,
    upload:   Option<UploadJobId>,
}

pub struct MediaSupervisor {
    db:         Db,
    media:      HashMap<AppMediaObjectId, MediaJobs>,
    uploads:    HashMap<UploadJobId, Addr<Uploader>>,
    downloads:  HashMap<DownloadJobId, Addr<Downloader>>,
    media_root: PathBuf,
    client:     reqwest::Client,
}

impl MediaSupervisor {
    pub fn new(cfg: MediaOpts, db: Db) -> anyhow::Result<MediaSupervisor> {
        let MediaOpts { media_root } = cfg;

        if !media_root.exists() {
            return Err(anyhow!("media root {media_root:?} does not exist"));
        }

        Ok(MediaSupervisor { db,
                             media: Default::default(),
                             uploads: HashMap::new(),
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

pub async fn init(cfg: MediaOpts, db: Db) -> anyhow::Result<Addr<MediaSupervisor>> {
    let service = MediaSupervisor::new(cfg, db)?;

    let addr = MEDIA_SUPERVISOR.get_or_init(move || Supervisor::start(move || service))
                               .clone();

    addr.send(RestartPendingUploadsDownloads).await??;

    Ok(addr)
}

impl MediaSupervisor {
    fn update(&mut self, ctx: &mut Context<Self>) {
        self.uploads.retain(|_, uploader| uploader.connected());
        self.downloads.retain(|_, downloader| downloader.connected());

        for media in self.media.values_mut() {
            if let Some(upload) = media.upload.as_ref() {
                if !self.uploads.contains_key(upload) {
                    media.upload = None;
                }
            }

            if let Some(download) = media.download.as_ref() {
                if !self.downloads.contains_key(download) {
                    media.download = None;
                }
            }
        }

        self.media
            .retain(|_, media| media.upload.is_some() || media.download.is_some());
    }

    fn notify_all_tasks(&mut self, media_id: AppMediaObjectId, ctx: &mut Context<Self>) {
        let tasks = self.tasks
                        .iter()
                        .filter_map(|(id, task)| task.contains(&media_id).then(|| id.clone()))
                        .collect::<HashSet<_>>();

        for task_id in tasks {
            let res = self.db.get_media_for_task(&task_id);

            match res {
                Ok(media) => self.issue_system_async(NotifyMediaTaskState { task_id, media }),
                Err(error) => {
                    error!(%error, %task_id, "Failed to notify session media");
                }
            }
        }
    }
}

impl Handler<NotifyDownloadProgress> for MediaSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyDownloadProgress, ctx: &mut Self::Context) -> Self::Result {
        let state = msg.state.clone();
        let media_id = msg.media_id.clone();

        self.db.update_media(&media_id, move |media| {
                   if let Some(download) = media.download.as_mut() {
                       download.state = state.clone();
                   }

                   Ok(())
               });

        self.notify_all_tasks(msg.media_id, ctx);

        if !msg.state.in_progress {
            self.downloads.remove(&msg.job_id);

            if let Some(media) = self.media.get_mut(&msg.media_id) {
                media.download = None;
            }
        }
    }
}

impl Handler<NotifyUploadProgress> for MediaSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyUploadProgress, ctx: &mut Self::Context) -> Self::Result {
        let state = msg.state.clone();
        let media_id = msg.media_id.clone();

        self.db.update_media(&media_id, move |media| {
                   if let Some(upload) = media.upload.as_mut() {
                       upload.state = state.clone();

                       if state.is_finished_ok() {
                           let mut path = PathBuf::new();
                           path.push(media.id.app_id.as_str());
                           path.push(media.id.media_id.as_str());

                           media.metadata = Some(upload.upload.metadata());
                           media.path = Some(path.to_string_lossy().to_string());
                       }
                   }

                   Ok(())
               });
        self.notify_all_tasks(msg.media_id, ctx);

        if !msg.state.in_progress {
            self.uploads.remove(&msg.job_id);

            if let Some(media) = self.media.get_mut(&msg.media_id) {
                media.upload = None;
            }
        }
    }
}

impl Handler<ImportMedia> for MediaSupervisor {
    type Result = anyhow::Result<()>;

    fn handle(&mut self, msg: ImportMedia, ctx: &mut Self::Context) -> Self::Result {
        self.db.save_media(MediaObject { id:       msg.media_id.clone(),
                                         metadata: Some(msg.import.metadata()),
                                         path:     Some(msg.import.path),
                                         download: None,
                                         upload:   None, })
    }
}

impl Handler<QueueDownload> for MediaSupervisor {
    type Result = anyhow::Result<()>;

    fn handle(&mut self, msg: QueueDownload, ctx: &mut Context<Self>) -> Self::Result {
        let QueueDownload { job_id,
                            media_id,
                            download, } = msg;

        let path = self.media_root
                       .join(media_id.app_id.to_string())
                       .join(media_id.media_id.to_string());

        if !path.exists() {
            return Err(anyhow!("Media file not found, cannot download"));
        }

        let downloader = Downloader::new(job_id, self.client.clone(), path, media_id.clone(), download)?;

        self.downloads.insert(job_id, Supervisor::start(move |_| downloader));

        Ok(())
    }
}

impl Handler<QueueUpload> for MediaSupervisor {
    type Result = anyhow::Result<()>;

    fn handle(&mut self, msg: QueueUpload, ctx: &mut Context<Self>) -> Self::Result {
        let app_media_id = msg.media_id.clone();

        if let Err(error) = self.db.create_default_media_if_not_exists(&app_media_id) {
            error!(%error, "failed to create default media for the download");
            return Err(error);
        }

        let QueueUpload { job_id,
                          session_id,
                          media_id,
                          upload, } = msg;

        let path = self.media_root
                       .join(media_id.app_id.to_string())
                       .join(media_id.media_id.to_string());

        let uploader = Uploader::new(job_id, self.client.clone(), path, session_id, media_id.clone(), upload)?;

        self.uploads.insert(job_id, Supervisor::start(move |_| uploader));

        Ok(())
    }
}

impl Handler<NotifyTaskSpec> for MediaSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyTaskSpec, ctx: &mut Self::Context) -> Self::Result {
        let task_media_ids = msg.spec.get_media_object_ids(&msg.task_id.app_id);
        self.tasks.insert(msg.task_id.clone(), task_media_ids.clone());

        let task_media_status = self.db.get_media_info(&task_media_ids)?;

        for media_id in task_media_ids {
            if task_media_status.get(&media_id)
                                .map(|media| media.upload.is_none())
                                .unwrap_or(true)
            {
                ctx.notify(QueueUpload { job_id:     UploadJobId::new(),
                                         media_id:   media_id.clone(),
                                         session_id: Some(msg.task_id.clone()),
                                         upload:     None, });
            }
        }
    }
}

impl Handler<RestartPendingUploadsDownloads> for MediaSupervisor {
    type Result = anyhow::Result<()>;

    fn handle(&mut self, msg: RestartPendingUploadsDownloads, ctx: &mut Self::Context) -> Self::Result {
        let mut delay = Duration::from_millis(100);

        let mut advance = || {
            delay = (delay + Duration::from_millis(100)).min(Duration::from_secs(5 * 60));
        };

        match self.db.get_pending_downloads_uploads() {
            Ok((pending_downloads, pending_uploads)) => {
                for (media_id, download) in pending_downloads {
                    ctx.notify_later(QueueDownload { job_id: DownloadJobId::new(),
                                                     media_id,
                                                     download },
                                     delay);

                    advance();
                }

                for (media_id, upload) in pending_uploads {
                    ctx.notify(QueueUpload { job_id: UploadJobId::new(),
                                             media_id,
                                             session_id: None,
                                             upload: Some(upload) },
                               delay);

                    advance();
                }

                Ok(())
            }
            Err(err) => {
                error!(%err, "failed to restart pending uploads/downloads");

                Err(err)
            }
        }
    }
}

impl Actor for MediaSupervisor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.restarting(ctx);
    }
}

impl Supervised for MediaSupervisor {
    fn restarting(&mut self, ctx: &mut <Self as Actor>::Context) {
        ctx.run_interval(Duration::from_millis(100), Self::update);

        self.subscribe_system_async::<NotifyDownloadProgress>(ctx);
        self.subscribe_system_async::<NotifyUploadProgress>(ctx);
        self.subscribe_system_async::<NotifyTaskSpec>(ctx);
    }
}
