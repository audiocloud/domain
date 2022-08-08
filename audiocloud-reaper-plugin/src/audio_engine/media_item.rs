use askama::Template;
use audiocloud_api::cloud::media::AppMedia;
use reaper_medium::{MediaItem, MediaItemTake, MediaTrack, Reaper};
use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::path::PathBuf;
use uuid::Uuid;

use audiocloud_api::newtypes::{AppId, AppMediaObjectId, MediaId};
use audiocloud_api::session::SessionTrackMedia;

use crate::audio_engine::media_track::AudioEngineMediaTrack;
use crate::audio_engine::project::AudioEngineProject;

#[derive(Debug)]
pub struct AudioEngineMediaItem {
    media_id:  MediaId,
    object_id: AppMediaObjectId,
    item_id:   Uuid,
    take_id:   Uuid,
    track:     MediaTrack,
    item:      MediaItem,
    take:      MediaItemTake,
    spec:      SessionTrackMedia,
    path:      Option<String>,
}

impl AudioEngineMediaItem {
    pub fn new(track: MediaTrack,
               media_root: &PathBuf,
               app_id: &AppId,
               media_id: MediaId,
               spec: SessionTrackMedia,
               media: &HashMap<AppMediaObjectId, String>)
               -> anyhow::Result<Self> {
        let object_id = spec.object_id.clone().for_app(app_id.clone());
        let path = media.get(&object_id)
                        .map(|path| media_root.join(path).to_string_lossy().to_string());

        let reaper = Reaper::get();
        let item = reaper.add_media_item_to_track(track)?;
        let take = reaper.add_take_to_media_item(item)?;

        let item_id = get_media_item_uuid(item)?;
        let take_id = get_media_item_take_uuid(item)?;

        Ok(Self { media_id,
                  object_id,
                  item_id,
                  take_id,
                  track,
                  item,
                  take,
                  spec,
                  path })
    }

    pub fn delete(&mut self) -> anyhow::Result<()> {
        let reaper = Reaper::get();
        reaper.delete_track_media_item(self.track, self.item)?;

        Ok(())
    }

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

fn get_media_item_uuid(media_item: MediaItem) -> anyhow::Result<Uuid> {
    let reaper = Reaper::get();
    let param = CString::new("GUID")?;
    let mut buffer = [0i8; 1024];

    unsafe {
        reaper.low()
              .GetSetMediaItemInfo_String(media_item.as_ptr(), param.into_raw(), buffer.as_mut_ptr(), false)?;

        let str = CString::from_raw(buffer.as_mut_ptr() as *mut i8).to_string_lossy()
                                                                   .to_string();

        Ok(Uuid::try_parse(&str[1..str.len() - 1])?)
    }
}

fn get_media_item_take_uuid(media_item_take: MediaItemTake) -> anyhow::Result<Uuid> {
    let reaper = Reaper::get();
    let param = CString::new("GUID")?;
    let mut buffer = [0i8; 1024];

    unsafe {
        reaper.low()
              .GetSetMediaItemTakeInfo_String(media_item_take.as_ptr(), param.into_raw(), buffer.as_mut_ptr(), false)?;

        let str = CString::from_raw(buffer.as_mut_ptr() as *mut i8).to_string_lossy()
                                                                   .to_string();

        Ok(Uuid::try_parse(&str[1..str.len() - 1])?)
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
    pub fn new(media: &'a AudioEngineMediaItem,
               track: &'a AudioEngineMediaTrack,
               project: &'a AudioEngineProject)
               -> Self {
        Self { media, track, project }
    }
}
