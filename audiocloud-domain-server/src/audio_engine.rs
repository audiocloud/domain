use std::collections::HashSet;

use audiocloud_api::audio_engine::command::AudioEngineCommand;
use audiocloud_api::newtypes::{AppTaskId, EngineId};

use crate::service::nats::get_nats_client;

#[derive(Clone)]
pub struct AudioEngineClient {
    id:       String,
    sessions: HashSet<AppTaskId>,
}

impl AudioEngineClient {
    pub fn new(id: String) -> Self {
        Self { id,
               sessions: Default::default() }
    }

    pub fn num_sessions(&self) -> usize {
        self.sessions.len()
    }
}

pub async fn request(engine_id: EngineId, request: AudioEngineCommand) -> anyhow::Result<()> {
    get_nats_client().request_audio_engine(&engine_id, request).await
}
