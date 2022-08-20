use crate::service::nats::get_nats_client;
use audiocloud_api::audio_engine::AudioEngineCommand;
use audiocloud_api::newtypes::AppSessionId;

use crate::service::session::session_audio_engine::SessionAudioEngineClient;

#[derive(Clone)]
pub struct AudioEngineClient {
    id: String,
}

impl AudioEngineClient {
    pub fn new(id: String) -> Self {
        Self { id }
    }

    pub fn for_session(&self, session_id: AppSessionId) -> SessionAudioEngineClient {
        SessionAudioEngineClient::new(self.clone(), session_id)
    }

    pub async fn request(&self, request: AudioEngineCommand) -> anyhow::Result<()> {
        get_nats_client().request_audio_engine(&self.id, request).await
    }
}
