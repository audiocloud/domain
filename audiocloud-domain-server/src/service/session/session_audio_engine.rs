use std::collections::HashMap;

use audiocloud_api::audio_engine::AudioEngineCommand;
use audiocloud_api::change::{DesiredSessionPlayState, PlaySession, RenderSession, SessionPlayState};
use audiocloud_api::cloud::apps::SessionSpec;
use audiocloud_api::cloud::domains::InstanceRouting;
use audiocloud_api::newtypes::{AppMediaObjectId, AppSessionId, FixedInstanceId};
use AudioEngineCommand::*;

use crate::audio_engine::AudioEngineClient;
use crate::tracker::RequestTracker;

#[derive(Clone)]
pub struct SessionAudioEngineClient {
    client:      AudioEngineClient,
    session_id:  AppSessionId,
    instances:   HashMap<FixedInstanceId, InstanceRouting>,
    media_ready: HashMap<AppMediaObjectId, String>,
    current:     DesiredSessionPlayState,
    tracker:     RequestTracker,
}

impl SessionAudioEngineClient {
    pub fn new(client: AudioEngineClient, session_id: AppSessionId) -> Self {
        Self { client,
               session_id,
               instances: Default::default(),
               media_ready: Default::default(),
               current: DesiredSessionPlayState::Stopped,
               tracker: Default::default() }
    }

    pub fn should_update(&mut self, actual: &SessionPlayState) -> bool {
        if !actual.satisfies(&self.current) {
            if self.tracker.should_retry() {
                return true;
            }
        } else if !self.tracker.is_completed() {
            self.tracker.complete();
        }

        false
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
            .request(Instances { session_id: self.session_id.clone(),
                                 instances:  self.instances.clone(), })
            .await
    }

    pub async fn sync_media_ready(&mut self) -> anyhow::Result<()> {
        self.client
            .request(Media { session_id:  self.session_id.clone(),
                             media_ready: self.media_ready.clone(), })
            .await
    }

    pub async fn set_spec(&mut self, spec: SessionSpec) -> anyhow::Result<()> {
        self.client
            .request(SetSpec { spec,
                               session_id: self.session_id.clone(),
                               instances: self.instances.clone(),
                               media_ready: self.media_ready.clone() })
            .await
    }

    pub async fn play(&mut self, play: PlaySession) -> anyhow::Result<()> {
        self.current = DesiredSessionPlayState::Play(play.clone());
        self.tracker.reset();
        Self::send_play(&self.client, self.session_id.clone(), play).await
    }

    async fn send_play(client: &AudioEngineClient, session_id: AppSessionId, play: PlaySession) -> anyhow::Result<()> {
        client.request(Play { play, session_id }).await
    }

    pub async fn render(&mut self, render: RenderSession) -> anyhow::Result<()> {
        self.current = DesiredSessionPlayState::Render(render.clone());
        self.tracker.reset();
        Self::send_render(&self.client, self.session_id.clone(), render).await
    }

    async fn send_render(client: &AudioEngineClient,
                         session_id: AppSessionId,
                         render: RenderSession)
                         -> anyhow::Result<()> {
        client.request(Render { render, session_id }).await
    }

    pub async fn stop(&mut self) -> anyhow::Result<()> {
        self.current = DesiredSessionPlayState::Stopped;
        self.tracker.reset();
        Self::send_stop(&self.client, self.session_id.clone()).await
    }

    async fn send_stop(client: &AudioEngineClient, session_id: AppSessionId) -> anyhow::Result<()> {
        client.request(Stop { session_id }).await
    }

    pub async fn update(self) -> anyhow::Result<()> {
        match self.current {
            DesiredSessionPlayState::Play(play) => Self::send_play(&self.client, self.session_id, play).await,
            DesiredSessionPlayState::Render(render) => Self::send_render(&self.client, self.session_id, render).await,
            DesiredSessionPlayState::Stopped => Self::send_stop(&self.client, self.session_id).await,
        }
    }
}
