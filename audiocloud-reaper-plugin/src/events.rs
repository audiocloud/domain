use std::collections::HashMap;

use audiocloud_api::{
    audio_engine::{AudioEngineCommand, AudioEngineEvent, CompressedAudio},
    change::PlayId,
    model::MultiChannelTimestampedValue,
};
use flume::Sender;

use crate::{control_surface::ReaperTrackId, streaming::StreamingConfig};

#[derive(Debug, PartialEq, Clone)]
pub enum ControlSurfaceEvent {
    EngineEvent(AudioEngineEvent),
    Metering(HashMap<ReaperTrackId, MultiChannelTimestampedValue>),
    SetupStreaming(Option<StreamingConfig>),
}

#[derive(Debug, PartialEq, Clone)]
pub enum AudioStreamingEvent {
    CompressedAudio { audio: CompressedAudio },
}

#[derive(Debug, PartialEq, Clone)]
pub enum ControlSurfaceCommand {
    Engine(AudioEngineCommand),
    StreamingSetupComplete(PlayId),
    StreamingSetupError(PlayId, String),
}

pub type ControlSurfaceCommandWithResultSender = (ControlSurfaceCommand, Sender<anyhow::Result<()>>);
