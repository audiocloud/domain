use askama::Template;
use reaper_medium::{MediaTrack, ProjectContext, Reaper};
use uuid::Uuid;

use audiocloud_api::newtypes::MixerId;
use audiocloud_api::session::{SessionFlowId, SessionMixer};

use crate::audio_engine::project::AudioEngineProject;
use crate::audio_engine::{append_track, beautify_chunk, delete_track, set_track_chunk};

#[derive(Debug)]
pub struct AudioEngineMixer {
    mixer_id:       MixerId,
    input_flow_id:  SessionFlowId,
    output_flow_id: SessionFlowId,
    input_id:       Uuid,
    output_id:      Uuid,
    input_track:    MediaTrack,
    output_track:   MediaTrack,
    spec:           SessionMixer,
}

impl AudioEngineMixer {
    pub(crate) fn delete(&self, context: ProjectContext) {
        delete_track(context, self.input_track);
        delete_track(context, self.output_track);
    }
}

impl AudioEngineMixer {
    pub fn new(project: &AudioEngineProject, mixer_id: MixerId, spec: SessionMixer) -> anyhow::Result<Self> {
        let input_flow_id = SessionFlowId::MixerInput(mixer_id.clone());
        let output_flow_id = SessionFlowId::MixerOutput(mixer_id.clone());

        let reaper = Reaper::get();

        project.focus()?;

        let (input_track, input_id) = append_track(&input_flow_id, project.context())?;
        let (output_track, output_id) = append_track(&output_flow_id, project.context())?;

        Ok(Self { mixer_id,
                  input_flow_id,
                  output_flow_id,
                  input_id,
                  output_id,
                  input_track,
                  output_track,
                  spec })
    }

    pub fn get_input_flow_id(&self) -> &SessionFlowId {
        &self.input_flow_id
    }

    pub fn get_output_flow_id(&self) -> &SessionFlowId {
        &self.output_flow_id
    }

    pub fn get_input_track(&self) -> MediaTrack {
        self.input_track
    }

    pub fn get_input_state_chunk(&self, project: &AudioEngineProject) -> anyhow::Result<String> {
        Ok(beautify_chunk(AudioEngineMixerInputTemplate { project, mixer: self }.render()?))
    }

    pub fn get_output_state_chunk(&self, project: &AudioEngineProject) -> anyhow::Result<String> {
        Ok(beautify_chunk(AudioEngineMixerOutputTemplate { project, mixer: self }.render()?))
    }

    pub fn update_state_chunk(&self, project: &AudioEngineProject) -> anyhow::Result<()> {
        set_track_chunk(self.input_track, &self.get_input_state_chunk(project)?)?;
        set_track_chunk(self.output_track, &self.get_output_state_chunk(project)?)?;

        Ok(())
    }
}

#[derive(Template)]
#[template(path = "audio_engine/mixer_track_input.txt")]
struct AudioEngineMixerInputTemplate<'a> {
    project: &'a AudioEngineProject,
    mixer:   &'a AudioEngineMixer,
}

#[derive(Template)]
#[template(path = "audio_engine/mixer_track_output.txt")]
struct AudioEngineMixerOutputTemplate<'a> {
    project: &'a AudioEngineProject,
    mixer:   &'a AudioEngineMixer,
}
