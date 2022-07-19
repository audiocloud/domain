use serde::{Deserialize, Serialize};

use audiocloud_api::change::SessionState;
use audiocloud_api::cloud::apps::SessionSpec;
use audiocloud_api::time::Timestamped;

use crate::data::reaper::ReaperState;

#[derive(Debug)]
pub struct Session {
    pub spec:   SessionSpec,
    pub state:  SessionState,
    pub reaper: Timestamped<Option<ReaperState>>,
}
