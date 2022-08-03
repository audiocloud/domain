use audiocloud_api::{
    audio_engine::CompressedAudio,
    change::{PlaySession, RenderId, RenderSession},
};

#[derive(Debug, PartialEq, Clone)]
pub enum ControlSurfaceEvent {
    RenderCancelled { render: RenderSession },
    RenderSuccess { render: RenderSession, output: String },
    RenderStart { render: RenderSession },
    PlayingStart { play: PlaySession },
    PlayingStopped { play: PlaySession },
    Seek { play: PlaySession },
}

#[derive(Debug, PartialEq, Clone)]
pub enum AudioStreamingEvent {
    CompressedAudio { audio: CompressedAudio },
}
