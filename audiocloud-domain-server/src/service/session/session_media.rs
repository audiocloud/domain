use std::collections::{HashMap, HashSet};

use audiocloud_api::media::MediaObject;
use audiocloud_api::newtypes::{AppMediaObjectId, MediaObjectId};

#[derive(Default)]
pub struct SessionMedia {
    pub media: HashMap<AppMediaObjectId, MediaObject>,
}

impl SessionMedia {
    pub fn waiting_for_media(&self) -> HashSet<AppMediaObjectId> {
        let mut rv = HashSet::new();

        for (object_id, object) in &self.media {
            if object.path.is_some() || object.upload.is_none() {
                rv.insert(object_id.clone());
            }
        }

        rv
    }
}
