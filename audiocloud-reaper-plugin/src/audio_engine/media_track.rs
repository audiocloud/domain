use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use askama::Template;
use reaper_medium::{MediaTrack, ProjectContext};
use uuid::Uuid;

use audiocloud_api::newtypes::{AppId, AppMediaObjectId, MediaId, MediaObjectId, TrackId};
use audiocloud_api::session::{
    SessionFlowId, SessionTimeSegment, SessionTrack, SessionTrackChannels, SessionTrackMedia, SessionTrackMediaFormat,
};

use crate::audio_engine;
use crate::audio_engine::media_item::{AudioEngineMediaItem, AudioEngineMediaItemTemplate};
use crate::audio_engine::project::AudioEngineProject;
use crate::audio_engine::{append_track, delete_track, set_track_chunk};

#[derive(Debug)]
pub struct AudioEngineMediaTrack {
    id:       Uuid,
    app_id:   AppId,
    track_id: TrackId,
    flow_id:  SessionFlowId,
    track:    MediaTrack,
    media:    HashMap<MediaId, AudioEngineMediaItem>,
    spec:     SessionTrack,
    root_dir: PathBuf,
}

impl AudioEngineMediaTrack {
    pub fn new(project: &AudioEngineProject,
               app_id: AppId,
               track_id: TrackId,
               spec: SessionTrack,
               existing_media: &HashMap<AppMediaObjectId, String>)
               -> anyhow::Result<Self> {
        project.focus()?;

        let root_dir = project.media_root_dir();

        let flow_id = SessionFlowId::TrackOutput(track_id.clone());

        let (track, id) = append_track(&flow_id, project.context())?;

        let mut media = HashMap::new();

        for (media_id, media_spec) in spec.media.clone() {
            media.insert(media_id.clone(),
                         AudioEngineMediaItem::new(track, &root_dir, &app_id, media_id, media_spec, existing_media)?);
        }

        let mut rv = Self { id,
                            app_id,
                            track_id,
                            flow_id,
                            track,
                            media,
                            spec,
                            root_dir };

        Ok(rv)
    }

    pub fn delete(self, context: ProjectContext) {
        delete_track(context, self.track);
    }

    pub fn get_flow_id(&self) -> &SessionFlowId {
        &self.flow_id
    }

    pub fn get_state_chunk(&self, project: &AudioEngineProject) -> anyhow::Result<String> {
        Ok(audio_engine::beautify_chunk(AudioEngineMediaTrackTemplate { project, track: self }.render()?))
    }

    pub fn on_media_updated(&mut self,
                            available: &HashMap<AppMediaObjectId, String>,
                            removed: &HashSet<AppMediaObjectId>)
                            -> bool {
        let mut rv = false;
        for media in self.media.values_mut() {
            if media.updated(&self.root_dir, available, removed) {
                rv |= true;
            }
        }

        rv
    }

    pub fn add_media(&mut self,
                     media_id: MediaId,
                     spec: SessionTrackMedia,
                     media: &HashMap<AppMediaObjectId, String>)
                     -> anyhow::Result<bool> {
        self.delete_media(&media_id)?;

        self.media.insert(media_id.clone(),
                          AudioEngineMediaItem::new(self.track, &self.root_dir, &self.app_id, media_id, spec, media)?);

        Ok(true)
    }

    pub fn delete_media(&mut self, media_id: &MediaId) -> anyhow::Result<()> {
        if let Some(mut media) = self.media.remove(media_id) {
            media.delete()?;
        }

        Ok(())
    }

    pub fn update_state_chunk(&self, project: &AudioEngineProject) -> anyhow::Result<()> {
        set_track_chunk(self.track, &self.get_state_chunk(project)?)?;

        Ok(())
    }
}

#[derive(Template)]
#[template(path = "audio_engine/media_track.txt")]
struct AudioEngineMediaTrackTemplate<'a> {
    project: &'a AudioEngineProject,
    track:   &'a AudioEngineMediaTrack,
}
