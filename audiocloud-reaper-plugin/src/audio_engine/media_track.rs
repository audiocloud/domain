use std::collections::HashMap;
use std::path::PathBuf;

use askama::Template;
use reaper_medium::{MediaTrack, ProjectContext};
use tracing::*;
use uuid::Uuid;

use audiocloud_api::common::model::MultiChannelValue;
use audiocloud_api::common::task::{NodePadId, TrackMedia, TrackNode, UpdateTaskTrackMedia};
use audiocloud_api::newtypes::{AppId, AppMediaObjectId, TrackMediaId, TrackNodeId};
use crate::audio_engine;
use crate::audio_engine::media_item::{AudioEngineMediaItem, AudioEngineMediaItemTemplate};
use crate::audio_engine::project::{AudioEngineProject, AudioEngineProjectTemplateSnapshot, get_track_peak_meters};
use crate::audio_engine::{append_track, delete_track, set_track_chunk};

#[derive(Debug)]
pub struct AudioEngineMediaTrack {
    id: TrackNodeId,
    track_id: Uuid,
    app_id:   AppId,
    flow_id: NodePadId,
    track:    MediaTrack,
    media:    HashMap<TrackMediaId, AudioEngineMediaItem>,
    spec: TrackNode,
    root_dir: PathBuf,
}

impl AudioEngineMediaTrack {
    #[instrument(skip_all, err)]
    pub fn new(project: &AudioEngineProject,
               app_id: AppId,
               track_id: TrackNodeId,
               spec: TrackNode,
               existing_media: &HashMap<AppMediaObjectId, String>)
               -> anyhow::Result<Self> {
        project.focus()?;

        let root_dir = project.shared_media_root_dir();

        let flow_id = NodePadId::TrackOutput(track_id.clone());

        let (track, id) = append_track(&flow_id, project.context())?;

        let mut media = HashMap::new();

        for (media_id, media_spec) in spec.media.clone() {
            media.insert(media_id.clone(),
                         AudioEngineMediaItem::new(track, &root_dir, &app_id, media_id, media_spec, existing_media)?);
        }

        let rv = Self { track_id: id,
                        app_id,
                        id: track_id,
                        flow_id,
                        track,
                        media,
                        spec,
                        root_dir };

        Ok(rv)
    }

    #[instrument(skip_all)]
    pub fn delete(self, context: ProjectContext) {
        delete_track(context, self.track);
    }

    pub fn get_flow_id(&self) -> &NodePadId {
        &self.flow_id
    }

    pub fn get_state_chunk(&self, project: &AudioEngineProjectTemplateSnapshot) -> anyhow::Result<String> {
        Ok(audio_engine::beautify_chunk(AudioEngineMediaTrackTemplate { project, track: self }.render()?))
    }

    pub fn on_media_updated(&mut self, available: &HashMap<AppMediaObjectId, String>) -> bool {
        let mut rv = false;
        for media in self.media.values_mut() {
            if media.on_media_updated(&self.root_dir, available) {
                rv |= true;
            }
        }

        rv
    }

    #[instrument(skip_all, err)]
    pub fn set_media_values(&mut self,
                            media_id: TrackMediaId,
                            update: UpdateTaskTrackMedia,
                            media: &HashMap<AppMediaObjectId, String>)
                            -> anyhow::Result<bool> {
        if let Some(media) = self.spec.media.get_mut(&media_id) {
            media.update(update.clone());
        }

        if let Some(media_item) = self.media.get_mut(&media_id) {
            media_item.update(update);
            Ok(media_item.on_media_updated(&self.root_dir, media))
        } else {
            Err(anyhow::anyhow!("No media item found for {}", media_id))
        }
    }

    #[instrument(skip_all, err)]
    pub fn add_media(&mut self,
                     media_id: TrackMediaId,
                     spec: TrackMedia,
                     media: &HashMap<AppMediaObjectId, String>)
                     -> anyhow::Result<bool> {
        self.delete_media(&media_id)?;

        self.media.insert(media_id.clone(),
                          AudioEngineMediaItem::new(self.track, &self.root_dir, &self.app_id, media_id, spec, media)?);

        Ok(true)
    }

    #[instrument(skip_all, err)]
    pub fn delete_media(&mut self, media_id: &TrackMediaId) -> anyhow::Result<bool> {
        if let Some(media) = self.media.remove(media_id) {
            media.delete()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    #[instrument(skip_all, err, fields(id = %self.id))]
    pub fn update_state_chunk(&self, project: &AudioEngineProjectTemplateSnapshot) -> anyhow::Result<()> {
        set_track_chunk(project.context(), self.track, &self.get_state_chunk(project)?)?;

        Ok(())
    }

    pub fn fill_peak_meters(&self, peaks: &mut HashMap<NodePadId, MultiChannelValue>) {
        peaks.insert(self.flow_id.clone(),
                     get_track_peak_meters(self.track, self.spec.channels.num_channels()));
    }
}

#[derive(Template)]
#[template(path = "audio_engine/media_track.txt")]
struct AudioEngineMediaTrackTemplate<'a> {
    project: &'a AudioEngineProjectTemplateSnapshot,
    track:   &'a AudioEngineMediaTrack,
}
