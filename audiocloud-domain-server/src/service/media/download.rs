use std::path::PathBuf;

use actix::{Actor, ActorContext, ActorFutureExt, Addr, Context, ContextFutureSpawner, Supervised, WrapFuture};
use actix_broker::BrokerIssue;
use chrono::Utc;
use serde_json::json;
use tokio::fs::File;
use tracing::*;

use audiocloud_api::common::media::{DownloadFromDomain, MediaJobState};
use audiocloud_api::newtypes::AppMediaObjectId;
use audiocloud_api::common::time::now;

use crate::service::media::messages::{DownloadJobId, NotifyDownloadProgress};

#[derive(Debug)]
pub struct Downloader {
    job_id:   DownloadJobId,
    media_id: AppMediaObjectId,
    download: DownloadFromDomain,
    source:   PathBuf,
    state:    MediaJobState,
    client:   reqwest::Client,
}

impl Downloader {
    pub fn new(job_id: DownloadJobId,
               client: reqwest::Client,
               source: PathBuf,
               media_id: AppMediaObjectId,
               download: DownloadFromDomain)
               -> anyhow::Result<Self> {
        let state = MediaJobState::default();

        Ok(Self { job_id,
                  media_id,
                  download,
                  source,
                  client,
                  state })
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
                 actor.state.error = None;
                 actor.state.in_progress = false;

                 actor.notify_supervisor();

                 ctx.stop();
             }
             Err(err) => {
                 warn!(%err, "download failed");

                 actor.state.error = Some(err.to_string());

                 actor.notify_supervisor();

                 actor.restarting(ctx);
             }
         })
         .spawn(ctx);
    }

    fn notify_supervisor(&mut self) {
        self.state.updated_at = now();

        self.issue_system_async(NotifyDownloadProgress { job_id:   self.job_id,
                                                         media_id: self.media_id.clone(),
                                                         state:    self.state.clone(), });
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
        self.state.progress = 0.0;

        if self.state.retry > 5 {
            warn!("final failure");

            self.state.in_progress = false;

            self.notify_supervisor();

            ctx.stop();
        } else {
            self.state.retry += 1;

            self.notify_supervisor();

            self.download(ctx);
        }
    }
}
