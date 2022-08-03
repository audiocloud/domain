use std::{collections::HashMap, path::PathBuf};

use askama::Template;
use audiocloud_api::{
    newtypes::{MediaId, TrackId},
    session::{SessionObjectId, SessionTrack, SessionTrackMedia, SessionTrackMediaFormat},
};
use uuid::Uuid;

use super::ReaperTrackId;

#[derive(Debug)]
pub struct ReaperTrack {
    uuid:     Uuid,
    track_id: TrackId,
    spec:     SessionTrack, // created from a SessionTrack
    media:    HashMap<MediaId, ReaperTrackMedia>,
}

#[derive(Debug)]
struct ReaperTrackMedia {
    uuid: Uuid,
    spec: SessionTrackMedia, // created from a SessionTrackMedia
    path: Option<PathBuf>,
}

#[derive(Template)]
#[template(path = "reaper_track.xml.jinja")]
struct ReaperTrackTemplate<'a>(&'a ReaperTrack);

impl<'a> ReaperTrackTemplate<'a> {
    fn quoted_name(&self) -> String {
        quote(ReaperTrackId::Output(SessionObjectId::Track(self.0.track_id.clone())).to_string())
    }

    fn uuid_string(&self) -> String {
        reaper_uuid_string(&self.0.uuid)
    }

    fn channels(&self) -> usize {
        self.0.spec.channels.num_channels()
    }
}

#[derive(Template)]
#[template(path = "reaper_track_media.xml.jinja")]
struct ReaperTrackMediaTemplate<'a>(&'a ReaperTrackMedia);

impl<'a> ReaperTrackMediaTemplate<'a> {
    fn uuid_string() -> String {
        reaper_uuid_string(&self.0.uuid)
    }

    fn timeline_start(&self) -> f64 {
        self.0.spec.timeline_segment.start
    }

    fn timeline_length(&self) -> f64 {
        self.0.spec.timeline_segment.length
    }

    fn media_start(&self) -> f64 {
        self.0.spec.media_segment.start
    }

    fn media_length(&self) -> f64 {
        self.0.spec.media_segment.length
    }

    fn media_source_location_exists(&self) -> bool {
        self.0.path.is_some()
    }

    fn media_source_location(&self) -> String {
        self.0.path.as_ref().unwrap().to_string_lossy().to_string()
    }

    fn media_source_type(&self) -> &'static str {
        match self.0.spec.format {
            SessionTrackMediaFormat::Wav => "WAV",
            SessionTrackMediaFormat::Mp3 => "MP3",
            SessionTrackMediaFormat::Flac => "FLAC",
            SessionTrackMediaFormat::WavPack => "WAVPACK",
        }
    }
}

fn quote(s: String) -> String {
    format!("\"{s}\"")
}

fn reaper_uuid_string(uuid: &Uuid) -> String {
    uuid.as_hyphenated().encode_upper(&mut Uuid::encode_buffer()).to_owned()
}
