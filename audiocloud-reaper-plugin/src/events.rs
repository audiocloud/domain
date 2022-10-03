use std::collections::HashMap;

use flume::Sender;

use audiocloud_api::audio_engine::command::EngineCommand;
use audiocloud_api::audio_engine::event::EngineEvent;
use audiocloud_api::audio_engine::CompressedAudio;
use audiocloud_api::common::model::MultiChannelTimestampedValue;
use audiocloud_api::common::task::NodePadId;

use crate::streaming::StreamingConfig;

#[derive(Debug, PartialEq, Clone)]
pub enum ControlSurfaceEvent {
    EngineEvent(EngineEvent),
    Metering(HashMap<NodePadId, MultiChannelTimestampedValue>),
    SetupStreaming(Option<StreamingConfig>),
}

#[derive(Debug, PartialEq, Clone)]
pub enum AudioStreamingEvent {
    CompressedAudio { audio: CompressedAudio },
}

pub type EngineCommandWithResultSender = (EngineCommand, Sender<anyhow::Result<()>>);
