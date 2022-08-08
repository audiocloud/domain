use askama::Template;
use audiocloud_api::cloud::media::AppMedia;
use reaper_medium::MediaItem;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use uuid::Uuid;

use audiocloud_api::newtypes::{AppId, AppMediaObjectId, MediaId};
use audiocloud_api::session::SessionTrackMedia;

use crate::audio_engine::project::AudioEngineProject;
use crate::audio_engine::media_track::AudioEngineMediaTrack;

#[derive(Debug)]
pub struct AudioEngineMediaItem {
    media_id:  MediaId,
    object_id: AppMediaObjectId,
    item_id:   Uuid,
    take_id:   Uuid,
    item:      MediaItem,
    spec:      SessionTrackMedia,
    path:      Option<String>,
}

impl AudioEngineMediaItem {
    pub fn updated(&mut self,
                   root_dir: &PathBuf,
                   available: &HashMap<AppMediaObjectId, String>,
                   removed: &HashSet<AppMediaObjectId>)
                   -> bool {
        if self.path.is_some() {
            if removed.contains(&self.object_id) {
                self.path = None;
                return true;
            }
        } else {
            if let Some(path) = available.get(&self.object_id) {
                self.path = Some(root_dir.join(path).to_str().unwrap().to_string());
                return true;
            }
        }

        false
    }
}

#[derive(Template)]
#[template(path = "audio_engine/media_item.txt")]
pub struct AudioEngineMediaItemTemplate<'a> {
    media:   &'a AudioEngineMediaItem,
    track:   &'a AudioEngineMediaTrack,
    project: &'a AudioEngineProject,
}

impl<'a> AudioEngineMediaItemTemplate<'a> {
    pub fn new(media: &'a AudioEngineMediaItem, track: &'a AudioEngineMediaTrack, project: &'a AudioEngineProject) -> Self {
        Self { media, track, project }
    }
}
