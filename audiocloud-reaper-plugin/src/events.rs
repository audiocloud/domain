use std::collections::HashMap;

use flume::Sender;

use audiocloud_api::common::task::NodePadId;
use audiocloud_api::audio_engine::CompressedAudio;
use audiocloud_api::audio_engine::command::AudioEngineCommand;
use audiocloud_api::audio_engine::event::AudioEngineEvent;
use audiocloud_api::common::model::MultiChannelTimestampedValue;

use crate::streaming::StreamingConfig;

#[derive(Debug, PartialEq, Clone)]
pub enum ControlSurfaceEvent {
    EngineEvent(AudioEngineEvent),
    Metering(HashMap<NodePadId, MultiChannelTimestampedValue>),
    SetupStreaming(Option<StreamingConfig>),
}

#[derive(Debug, PartialEq, Clone)]
pub enum AudioStreamingEvent {
    CompressedAudio { audio: CompressedAudio },
}

pub type AudioEngineCommandWithResultSender = (AudioEngineCommand, Sender<anyhow::Result<()>>);
