use std::collections::HashMap;

use flume::Sender;

use audiocloud_api::session::SessionFlowId;
use audiocloud_api::{
    audio_engine::{AudioEngineCommand, AudioEngineEvent, CompressedAudio},
    model::MultiChannelTimestampedValue,
};

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

pub type AudioEngineCommandWithResultSender = (AudioEngineCommand, Sender<anyhow::Result<()>>);
