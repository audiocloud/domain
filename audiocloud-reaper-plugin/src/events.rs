use std::collections::HashMap;

use flume::Sender;

use audiocloud_api::{
    audio_engine::{AudioEngineCommand, AudioEngineEvent, CompressedAudio},
    change::PlayId,
    model::MultiChannelTimestampedValue,
};
use audiocloud_api::session::SessionFlowId;

use crate::streaming::StreamingConfig;

#[derive(Debug, PartialEq, Clone)]
pub enum ControlSurfaceEvent {
    EngineEvent(AudioEngineEvent),
    Metering(HashMap<SessionFlowId, MultiChannelTimestampedValue>),
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
