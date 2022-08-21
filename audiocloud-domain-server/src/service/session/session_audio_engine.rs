use std::collections::HashMap;

use audiocloud_api::audio_engine::AudioEngineCommand;
use audiocloud_api::change::{PlaySession, RenderSession};
use audiocloud_api::cloud::apps::SessionSpec;
use audiocloud_api::cloud::domains::InstanceRouting;
use audiocloud_api::newtypes::{AppMediaObjectId, AppSessionId, FixedInstanceId};

use crate::audio_engine::AudioEngineClient;

#[derive(Clone)]
pub struct SessionAudioEngineClient {
    client:      AudioEngineClient,
    session_id:  AppSessionId,
    instances:   HashMap<FixedInstanceId, InstanceRouting>,
    media_ready: HashMap<AppMediaObjectId, String>,
}

impl SessionAudioEngineClient {
    pub fn new(client: AudioEngineClient, session_id: AppSessionId) -> Self {
        Self { client,
               session_id,
               instances: Default::default(),
               media_ready: Default::default() }
    }

    pub async fn set_media_ready(&mut self,
                                 media_object_id: AppMediaObjectId,
                                 media_ready_path: String)
                                 -> anyhow::Result<()> {
        self.media_ready.insert(media_object_id, media_ready_path);
        self.sync_media_ready().await
    }

    pub async fn set_instances(&mut self, instances: HashMap<FixedInstanceId, InstanceRouting>) -> anyhow::Result<()> {
        self.instances = instances;
        self.sync_instances().await
    }

    pub async fn sync_instances(&mut self) -> anyhow::Result<()> {
        self.client
            .request(AudioEngineCommand::Instances { session_id: self.session_id.clone(),
                                                     instances:  self.instances.clone(), })
            .await
    }

    pub async fn sync_media_ready(&mut self) -> anyhow::Result<()> {
        self.client
            .request(AudioEngineCommand::Media { session_id:  self.session_id.clone(),
                                                 media_ready: self.media_ready.clone(), })
            .await
    }

    pub async fn set_spec(&mut self, spec: SessionSpec) -> anyhow::Result<()> {
        self.client
            .request(AudioEngineCommand::SetSpec { session_id: self.session_id.clone(),
                                                   spec,
                                                   instances: self.instances.clone(),
                                                   media_ready: self.media_ready.clone() })
            .await
    }

    pub async fn play(&mut self, play: PlaySession) -> anyhow::Result<()> {
        self.client
            .request(AudioEngineCommand::Play { session_id: self.session_id.clone(),
                                                play })
            .await
    }

    pub async fn render(&mut self, render: RenderSession) -> anyhow::Result<()> {
        self.client
            .request(AudioEngineCommand::Render { session_id: self.session_id.clone(),
                                                  render })
            .await
    }

    pub async fn stop(&mut self) -> anyhow::Result<()> {
        self.client
            .request(AudioEngineCommand::Stop { session_id: self.session_id.clone(), })
            .await
    }
}
