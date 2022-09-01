use std::path::PathBuf;

use actix::{Actor, ActorContext, ActorFutureExt, Addr, Context, ContextFutureSpawner, Supervised, WrapFuture};
use futures::TryStreamExt;
use serde_json::json;
use tokio::fs::File;
use tokio_util::io::StreamReader;
use tracing::*;

use audiocloud_api::media::UploadToDomain;
use audiocloud_api::newtypes::AppMediaObjectId;

use crate::service::media::messages::{MediaJobState, NotifyUploadProgress};
use crate::service::media::MediaSupervisor;

#[derive(Debug)]
pub struct Uploader {
    supervisor:  Addr<MediaSupervisor>,
    media_id:    AppMediaObjectId,
    upload:      UploadToDomain,
    destination: PathBuf,
    client:      reqwest::Client,
    retry_count: usize,
}

impl Uploader {
    pub fn new(supervisor: Addr<MediaSupervisor>,
               client: reqwest::Client,
               destination: PathBuf,
               media_id: AppMediaObjectId,
               upload: UploadToDomain)
               -> anyhow::Result<Self> {
        Ok(Self { supervisor,
                  media_id,
                  upload,
                  destination,
                  client,
                  retry_count: 0 })
    }

    fn upload(&mut self, ctx: &mut Context<Self>) {
        let destination = self.destination.clone();
        let client = self.client.clone();
        let upload = self.upload.clone();
        let id = self.media_id.clone();

        async move {
            let mut file = File::create(&destination).await?;

            let stream = client.get(&upload.url)
                               .send()
                               .await?
                               .bytes_stream()
                               .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err));

            let mut stream = StreamReader::new(stream);

            tokio::io::copy(&mut stream, &mut file).await?;

            if let Some(notify_url) = upload.notify_url {
                client.post(&notify_url)
                      .json(&json!({
                                "context": &upload.context,
                                "id": &id,
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
                 warn!(%err, "upload failed");

                 actor.restarting(ctx);
             }
         })
         .spawn(ctx);
    }

    fn notify_supervisor(&self, state: MediaJobState) {
        self.supervisor
            .do_send(NotifyUploadProgress { media_id: self.media_id.clone(),
                                            state });
    }
}

impl Actor for Uploader {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.restarting(ctx);
    }
}

impl Supervised for Uploader {
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

            self.upload(ctx);
        }
    }
}
