use crate::tracker::RequestTracker;
use audiocloud_api::cloud::apps::SessionSpec;

pub struct SessionAudioEngine {
    tracker: RequestTracker,
}

impl SessionAudioEngine {
    pub fn set_spec(&mut self, spec: SessionSpec) {

    }
}
