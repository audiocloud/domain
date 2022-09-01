use actix::Message;
use audiocloud_api::media::{DownloadFromDomain, UploadToDomain};
use audiocloud_api::newtypes::AppMediaObjectId;

#[derive(Message)]
#[rtype(result = "anyhow::Result<()>")]
pub struct QueueDownload {
    pub media_id: AppMediaObjectId,
    pub download: DownloadFromDomain,
}

#[derive(Message)]
#[rtype(result = "anyhow::Result<()>")]
pub struct QueueUpload {
    pub media_id: AppMediaObjectId,
    pub upload:   UploadToDomain,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct NotifyDownloadProgress {
    pub media_id: AppMediaObjectId,
    pub state:    MediaJobState,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct NotifyUploadProgress {
    pub media_id: AppMediaObjectId,
    pub state:    MediaJobState,
}

pub enum MediaJobState {
    Started,
    Retrying { count: usize },
    Finished { successfully: bool },
}
