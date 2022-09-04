use actix::Message;
use derive_more::{Display, From, FromStr};
use uuid::Uuid;

use audiocloud_api::media::{DownloadFromDomain, ImportToDomain, MediaJobState, UploadToDomain};
use audiocloud_api::newtypes::{AppMediaObjectId, AppSessionId};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Display, Hash, From, FromStr)]
#[repr(transparent)]
pub struct UploadJobId(Uuid);

impl UploadJobId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Display, Hash, From, FromStr)]
#[repr(transparent)]
pub struct DownloadJobId(Uuid);

impl DownloadJobId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Message)]
#[rtype(result = "anyhow::Result<()>")]
pub struct RestartPendingUploadsDownloads;

#[derive(Message)]
#[rtype(result = "anyhow::Result<()>")]
pub struct QueueDownload {
    pub job_id:   DownloadJobId,
    pub media_id: AppMediaObjectId,
    pub download: DownloadFromDomain,
}

#[derive(Message)]
#[rtype(result = "anyhow::Result<()>")]
pub struct QueueUpload {
    pub job_id:     UploadJobId,
    pub session_id: Option<AppSessionId>,
    pub media_id:   AppMediaObjectId,
    pub upload:     Option<UploadToDomain>,
}

#[derive(Message)]
#[rtype(result = "anyhow::Result<()>")]
pub struct ImportMedia {
    pub media_id: AppMediaObjectId,
    pub import:   ImportToDomain,
}

#[derive(Message, Clone)]
#[rtype(result = "()")]
pub struct NotifyDownloadProgress {
    pub job_id:   DownloadJobId,
    pub media_id: AppMediaObjectId,
    pub state:    MediaJobState,
}

#[derive(Message, Clone)]
#[rtype(result = "()")]
pub struct NotifyUploadProgress {
    pub job_id:   UploadJobId,
    pub media_id: AppMediaObjectId,
    pub state:    MediaJobState,
}
