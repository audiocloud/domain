use futures::TryStreamExt;
use serde_json::json;
use tokio_util::io::StreamReader;

use audiocloud_api::media::{DownloadFromDomain, UploadToDomain};
use audiocloud_api::newtypes::AppMediaObjectId;

#[derive(Clone)]
pub struct Service {
    client: reqwest::Client,
}

impl Service {
    /// Download a file from the domain media server and optionally notify the requester
    ///
    /// # Arguments
    ///
    /// * `id`: the ID of the file to be downloaded
    /// * `source`: local path of the file to download
    /// * `download`: details of the download request
    ///
    /// returns: Result<(), Error>
    pub async fn download_from_domain(&self,
                                      id: AppMediaObjectId,
                                      source: String,
                                      download: DownloadFromDomain)
                                      -> anyhow::Result<()> {
        self.client
            .put(&download.url)
            .body(tokio::fs::File::open(&source).await?)
            .send()
            .await?;

        if let Some(notify_url) = download.notify_url {
            self.client
                .post(&notify_url)
                .json(&json!({
                          "context": download.context,
                          "id": id,
                      }))
                .send()
                .await?;
        }

        Ok(())
    }

    /// Upload a file from a remote source to the media server and optionally notify the requester
    ///
    /// # Arguments
    ///
    /// * `id`: the ID of the file to be uploaded
    /// * `destination`: local path of the file to overwrite
    /// * `upload`: details of the upload request
    ///
    /// returns: Result<(), Error>
    pub async fn upload_to_domain(&self,
                                  id: AppMediaObjectId,
                                  destination: String,
                                  upload: UploadToDomain)
                                  -> anyhow::Result<()> {
        let mut file = tokio::fs::File::open(&destination).await?;

        let stream = self.client
                         .get(&upload.url)
                         .send()
                         .await?
                         .bytes_stream()
                         .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err));

        let mut stream = StreamReader::new(stream);

        tokio::io::copy(&mut stream, &mut file).await?;

        if let Some(notify_url) = upload.notify_url {
            self.client
                .post(&notify_url)
                .json(&json!({
                          "context": upload.context,
                          "id": id,
                      }))
                .send()
                .await?;
        }

        Ok(())
    }
}
