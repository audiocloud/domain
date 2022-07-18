use serde::{Deserialize, Serialize};

use audiocloud_api::change::SessionState;
use audiocloud_api::cloud::apps::SessionSpec;
use audiocloud_api::newtypes::AppSessionId;
use audiocloud_api::time::Timestamped;

#[derive(Serialize, Deserialize, Debug)]
pub struct Session {
    pub _id:    AppSessionId,
    pub spec:   SessionSpec,
    pub state:  SessionState,
    pub reaper: Timestamped<Option<String>>,
}
