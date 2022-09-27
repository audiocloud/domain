use std::io;
use std::path::PathBuf;

use actix::{Actor, ActorContext, ActorFutureExt, Context, ContextFutureSpawner, Supervised, WrapFuture};
use actix_broker::BrokerIssue;
use futures::TryStreamExt;
use serde_json::json;
use tokio::fs::File;
use tokio_util::io::StreamReader;
use tracing::*;

use audiocloud_api::common::media::{MediaJobState, UploadToDomain};
use audiocloud_api::common::time::now;
use audiocloud_api::newtypes::{AppMediaObjectId, AppTaskId};

use crate::media::messages::{NotifyUploadProgress, UploadJobId};

#[derive(Debug)]
pub struct Uploader {
    job_id:      UploadJobId,
    media_id:    AppMediaObjectId,
    session_id:  Option<AppTaskId>,
    upload:      Option<UploadToDomain>,
    destination: PathBuf,
    client:      reqwest::Client,
    state:       MediaJobState,
}

impl Uploader {
    pub fn new(job_id: UploadJobId,
               client: reqwest::Client,
               destination: PathBuf,
               session_id: Option<AppTaskId>,
               media_id: AppMediaObjectId,
               upload: Option<UploadToDomain>)
               -> anyhow::Result<Self> {
        let state = MediaJobState::default();

        Ok(Self { job_id,
                  media_id,
                  session_id,
                  upload,
                  destination,
                  client,
                  state })
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
                None => {
                    get_cloud_client().get_media_as_upload(session_id.as_ref(), &media_id)
                                      .await?
                }
            };

            if let Some(media) = db.get_media(&media_id).await? {
                match (media.path.as_ref(), media.metadata.as_ref()) {
                    (Some(path), Some(metadata)) => {
                        // TODO: more checks, for example media hash?
                        // TODO: actually check the on-disk size, not DB size?

                        if metadata.bytes == upload.bytes {
                            debug!(%media_id, "Media already uploaded");
                            return Ok(());
                        }
                    }
                    _ => {}
                }
            }

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
                 actor.state.error = None;
                 actor.state.in_progress = false;

                 actor.notify_supervisor();

                 ctx.stop();
             }
             Err(err) => {
                 warn!(%err, "upload failed");

                 actor.restarting(ctx);
             }
         })
         .spawn(ctx);
    }

    fn notify_supervisor(&mut self) {
        self.state.updated_at = now();
        self.issue_system_async(NotifyUploadProgress { job_id:   self.job_id,
                                                       media_id: self.media_id.clone(),
                                                       state:    self.state.clone(), });
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
        if self.state.retry > 5 {
            debug!("final failure");

            self.notify_supervisor();

            ctx.stop();
        } else {
            self.state.retry += 1;

            self.notify_supervisor();

            self.upload(ctx);
        }
    }
}
