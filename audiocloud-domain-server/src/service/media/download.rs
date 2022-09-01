use std::path::PathBuf;

use actix::{Actor, ActorContext, ActorFutureExt, Addr, Context, ContextFutureSpawner, Supervised, WrapFuture};
use serde_json::json;
use tokio::fs::File;
use tracing::*;

use audiocloud_api::media::DownloadFromDomain;
use audiocloud_api::newtypes::AppMediaObjectId;

use crate::service::media::messages::{MediaJobState, NotifyDownloadProgress, TerminateMediaJob};
use crate::service::media::MediaSupervisor;

#[derive(Debug)]
pub struct Downloader {
    supervisor:  Addr<MediaSupervisor>,
    media_id:    AppMediaObjectId,
    download:    DownloadFromDomain,
    source:      PathBuf,
    client:      reqwest::Client,
    retry_count: usize,
}

impl Downloader {
    pub fn new(supervisor: Addr<MediaSupervisor>,
               client: reqwest::Client,
               source: PathBuf,
               media_id: AppMediaObjectId,
               download: DownloadFromDomain)
               -> anyhow::Result<Self> {
        Ok(Self { media_id,
                  download,
                  source,
                  client,
                  supervisor,
                  retry_count: 0 })
    }

    #[instrument(skip(ctx))]
    fn download(&self, ctx: &mut Context<Self>) {
        debug!("starting download");

        let source = self.source.clone();
        let download = self.download.clone();
        let client = self.client.clone();
        let media_id = self.media_id.clone();

        async move {
            client.put(&download.url)
                  .body(File::open(&source).await?)
                  .send()
                  .await?;

            if let Some(notify_url) = &download.notify_url {
                client.post(notify_url)
                      .json(&json!({
                                "context": &download.context,
                                "id": &media_id,
                            }))
                      .send()
                      .await?;
            }

            Ok::<_, anyhow::Error>(())
        }.into_actor(self)
         .map(|res, actor, ctx| match res {
             Ok(_) => {
                 actor.notify_supervisor(MediaJobState::Finished { successfully: true });

                 ctx.stop();
             }
             Err(err) => {
                 warn!(%err, "download failed");

                 actor.restarting(ctx);
             }
         })
         .spawn(ctx);
    }

    fn notify_supervisor(&self, state: MediaJobState) {
        self.supervisor
            .do_send(NotifyDownloadProgress { media_id: self.media_id.clone(),
                                              state });
    }
}

impl Actor for Downloader {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.restarting(ctx);
    }
}

impl Supervised for Downloader {
    #[instrument(skip(ctx))]
    fn restarting(&mut self, ctx: &mut <Self as Actor>::Context) {
        if self.retry_count > 5 {
            debug!("final failure");

            self.notify_supervisor(MediaJobState::Finished { successfully: false });

            ctx.stop();
        } else {
            self.notify_supervisor(if self.retry_count == 0 {
                                       MediaJobState::Started
                                   } else {
                                       MediaJobState::Retrying { count: self.retry_count, }
                                   });

            self.retry_count += 1;

            self.download(ctx);
        }
    }
}
