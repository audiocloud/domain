use std::io;
use std::path::PathBuf;

use actix::{Actor, ActorContext, ActorFutureExt, Addr, Context, ContextFutureSpawner, Supervised, WrapFuture};
use futures::TryStreamExt;
use serde_json::json;
use tokio::fs::File;
use tokio_util::io::StreamReader;
use tracing::*;

use crate::service::cloud::get_cloud_client;
use audiocloud_api::media::UploadToDomain;
use audiocloud_api::newtypes::{AppMediaObjectId, AppSessionId};

use crate::service::media::messages::{MediaJobState, NotifyUploadProgress};
use crate::service::media::MediaSupervisor;

#[derive(Debug)]
pub struct Uploader {
    supervisor:  Addr<MediaSupervisor>,
    media_id:    AppMediaObjectId,
    session_id:  AppSessionId,
    upload:      Option<UploadToDomain>,
    destination: PathBuf,
    client:      reqwest::Client,
    retry_count: usize,
}

impl Uploader {
    pub fn new(supervisor: Addr<MediaSupervisor>,
               client: reqwest::Client,
               destination: PathBuf,
               session_id: AppSessionId,
               media_id: AppMediaObjectId,
               upload: Option<UploadToDomain>)
               -> anyhow::Result<Self> {
        Ok(Self { supervisor,
                  media_id,
                  session_id,
                  upload,
                  destination,
                  client,
                  retry_count: 0 })
    }

    fn upload(&mut self, ctx: &mut Context<Self>) {
        let destination = self.destination.clone();
        let client = self.client.clone();
        let session_id = self.session_id.clone();
        let media_id = self.media_id.clone();
        let upload = self.upload.clone();

        async move {
            let upload = match upload {
                Some(upload) => upload,
                None => get_cloud_client().get_media_as_upload(&session_id, &media_id).await?,
            };

            let mut file = File::create(&destination).await?;

            let stream = client.get(&upload.url)
                               .send()
                               .await?
                               .bytes_stream()
                               .map_err(|err| io::Error::new(io::ErrorKind::Other, err));

            let mut stream = StreamReader::new(stream);

            tokio::io::copy(&mut stream, &mut file).await?;

            if let Some(notify_url) = upload.notify_url {
                client.post(&notify_url)
                      .json(&json!({
                                "context": &upload.context,
                                "session_id": &session_id,
                                "media_id": &media_id,
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
