#![allow(unused_variables)]

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use actix::fut::LocalBoxActorFuture;
use actix::{
    Actor, ActorFutureExt, Addr, AsyncContext, Context, ContextFutureSpawner, Handler, Supervised, Supervisor,
    WrapFuture,
};
use actix_broker::{BrokerIssue, BrokerSubscribe};
use anyhow::anyhow;
use clap::Args;
use once_cell::sync::OnceCell;
use tracing::error;

use audiocloud_api::common::media::MediaObject;
use audiocloud_api::newtypes::{AppMediaObjectId, AppTaskId};
use download::Downloader;
use messages::{QueueDownload, QueueUpload};
use upload::Uploader;

use crate::data::MediaDatabase;
use crate::service::media::messages::{
    DownloadJobId, ImportMedia, NotifyDownloadProgress, NotifyUploadProgress, RestartPendingUploadsDownloads,
    UploadJobId,
};
use crate::service::session::messages::{NotifyMediaSessionState, NotifySessionSpec};

pub mod download;
pub mod messages;
pub mod upload;

static MEDIA_SUPERVISOR: OnceCell<Addr<MediaSupervisor>> = OnceCell::new();

struct MediaJobs {
    download: Option<DownloadJobId>,
    upload:   Option<UploadJobId>,
}

pub struct MediaSupervisor {
    media:      HashMap<AppMediaObjectId, MediaJobs>,
    uploads:    HashMap<UploadJobId, Addr<Uploader>>,
    downloads:  HashMap<DownloadJobId, Addr<Downloader>>,
    sessions:   HashMap<AppTaskId, HashSet<AppMediaObjectId>>,
    media_root: PathBuf,
    client:     reqwest::Client,
}

impl MediaSupervisor {
    pub fn new(cfg: MediaOpts) -> anyhow::Result<MediaSupervisor> {
        let MediaOpts { media_root } = cfg;

        if !media_root.exists() {
            return Err(anyhow!("media root {media_root:?} does not exist"));
        }

        Ok(MediaSupervisor { media: Default::default(),
                             uploads: HashMap::new(),
                             downloads: HashMap::new(),
                             client: reqwest::Client::new(),
                             sessions: Default::default(),
                             media_root })
    }
}

#[derive(Args)]
pub struct MediaOpts {
    #[clap(long, env, default_value = "media")]
    pub media_root: PathBuf,
}

pub async fn init(cfg: MediaOpts) -> anyhow::Result<Addr<MediaSupervisor>> {
    let service = MediaSupervisor::new(cfg)?;

    let addr = MEDIA_SUPERVISOR.get_or_init(move || service.start()).clone();

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

    fn notify_all_sessions(&mut self, media_id: AppMediaObjectId, ctx: &mut Context<Self>) {
        let sessions = self.sessions
                           .iter()
                           .filter_map(|(id, session)| session.contains(&media_id).then(|| id.clone()))
                           .collect::<HashSet<_>>();

        for session_id in sessions {
            {
                let session_id = session_id.clone();
                async move {
                    let db = MediaDatabase::default();
                    db.get_media_for_session(&session_id).await
                }
            }.into_actor(self)
             .map(move |res, actor, ctx| match res {
                 Ok(media) => actor.issue_system_async(NotifyMediaSessionState { session_id, media }),
                 Err(err) => {
                     error!(%err, %session_id, "failed to notify session media");
                 }
             })
             .wait(ctx);
        }
    }
}

impl Handler<NotifyDownloadProgress> for MediaSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyDownloadProgress, ctx: &mut Self::Context) -> Self::Result {
        let state = msg.state.clone();
        let media_id = msg.media_id.clone();

        async move {
            let db = MediaDatabase::default();
            db.update_media(&media_id, move |media| {
                  if let Some(download) = media.download.as_mut() {
                      download.state = state.clone();
                  }

                  Ok(())
              })
              .await
        }.into_actor(self)
         .map(move |_, actor, ctx| actor.notify_all_sessions(msg.media_id, ctx))
         .wait(ctx);

        if !msg.state.in_progress {
            self.downloads.remove(&msg.job_id);
        }
    }
}

impl Handler<NotifyUploadProgress> for MediaSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyUploadProgress, ctx: &mut Self::Context) -> Self::Result {
        let state = msg.state.clone();
        let media_id = msg.media_id.clone();

        async move {
            let db = MediaDatabase::default();
            db.update_media(&media_id, move |media| {
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
              })
              .await
        }.into_actor(self)
         .map(move |_, actor, ctx| actor.notify_all_sessions(msg.media_id, ctx))
         .wait(ctx);

        if !msg.state.in_progress {
            self.uploads.remove(&msg.job_id);
        }
    }
}

impl Handler<ImportMedia> for MediaSupervisor {
    type Result = LocalBoxActorFuture<Self, anyhow::Result<()>>;

    fn handle(&mut self, msg: ImportMedia, ctx: &mut Self::Context) -> Self::Result {
        async move {
            let db = MediaDatabase::default();
            db.save_media(MediaObject { id:       msg.media_id.clone(),
                                        metadata: Some(msg.import.metadata()),
                                        path:     Some(msg.import.path),
                                        download: None,
                                        upload:   None, })
              .await
        }.into_actor(self)
         .boxed_local()
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
        async move {
            let db = MediaDatabase::default();
            db.create_default_media_if_not_exists(&app_media_id).await
        }.into_actor(self)
         .map(|res, _, _| {
             if let Err(err) = res {
                 error!(%err, "failed to create default media for the download");
             }
         })
         .wait(ctx);

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

impl Handler<NotifySessionSpec> for MediaSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifySessionSpec, ctx: &mut Self::Context) -> Self::Result {
        let session_media_ids = msg.spec.get_media_object_ids(&msg.session_id.app_id);
        self.sessions.insert(msg.session_id.clone(), session_media_ids.clone());

        async move {
            let db = MediaDatabase::default();
            let owned_media_ids = db.get_media_ids_for_session(&msg.session_id).await?;

            // find diff
            let to_add = session_media_ids.difference(&owned_media_ids);

            let rv = to_add.map(|app_media_id| QueueUpload { job_id:     UploadJobId::new(),
                                                             media_id:   app_media_id.clone(),
                                                             session_id: Some(msg.session_id.clone()),
                                                             upload:     None, })
                           .collect::<Vec<_>>();
            
            db.set_media_files_for_session(&msg.session_id, session_media_ids.clone())
              .await?;

            Ok::<_, anyhow::Error>(rv)
        }.into_actor(self)
         .map(|result, actor, ctx| match result {
             Ok(jobs) => {
                 for job in jobs {
                     let _ = actor.handle(job, ctx);
                 }
             }
             Err(err) => {
                 error!(%err, "failed to set session media");
             }
         })
         .wait(ctx);
    }
}

impl Handler<RestartPendingUploadsDownloads> for MediaSupervisor {
    type Result = LocalBoxActorFuture<Self, anyhow::Result<()>>;

    fn handle(&mut self, msg: RestartPendingUploadsDownloads, ctx: &mut Self::Context) -> Self::Result {
        async move {
            let db = MediaDatabase::default();
            db.get_pending_downloads_uploads().await
        }.into_actor(self)
         .map(|res, _, ctx| match res {
             Ok((pending_downloads, pending_uploads)) => {
                 for (media_id, download) in pending_downloads {
                     ctx.notify(QueueDownload { job_id: DownloadJobId::new(),
                                                media_id,
                                                download });
                 }

                 for (media_id, upload) in pending_uploads {
                     ctx.notify(QueueUpload { job_id: UploadJobId::new(),
                                              media_id,
                                              session_id: None,
                                              upload: Some(upload) });
                 }

                 Ok(())
             }
             Err(err) => {
                 error!(%err, "failed to restart pending uploads/downloads");

                 Err(err)
             }
         })
         .boxed_local()
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
        self.subscribe_system_async::<NotifySessionSpec>(ctx);
    }
}
