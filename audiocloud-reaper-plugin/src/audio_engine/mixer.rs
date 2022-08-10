use std::collections::HashMap;
use std::ffi::CStr;
use std::path::PathBuf;

use askama::Template;
use reaper_medium::{MediaTrack, ProjectContext, Reaper, TrackAttributeKey};
use tracing::warn;
use uuid::Uuid;

use audiocloud_api::change::RenderSession;
use audiocloud_api::model::MultiChannelValue;
use audiocloud_api::newtypes::MixerId;
use audiocloud_api::session::{SessionFlowId, SessionMixer};

use crate::audio_engine::project::{
    get_track_peak_meters, set_track_master_send, AudioEngineProject, AudioEngineProjectTemplateSnapshot,
};
use crate::audio_engine::ConnectionTemplate;
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

    pub fn get_input_state_chunk(&self, project: &AudioEngineProjectTemplateSnapshot) -> anyhow::Result<String> {
        Ok(beautify_chunk(AudioEngineMixerInputTemplate { project, mixer: self }.render()?))
    }

    pub fn get_output_state_chunk(&self, project: &AudioEngineProjectTemplateSnapshot) -> anyhow::Result<String> {
        Ok(beautify_chunk(AudioEngineMixerOutputTemplate { project, mixer: self }.render()?))
    }

    pub fn update_state_chunk(&self, project: &AudioEngineProjectTemplateSnapshot) -> anyhow::Result<()> {
        set_track_chunk(self.input_track, &self.get_input_state_chunk(project)?)?;
        set_track_chunk(self.output_track, &self.get_output_state_chunk(project)?)?;

        Ok(())
    }

    pub fn fill_peak_meters(&self, peaks: &mut HashMap<SessionFlowId, MultiChannelValue>) {
        peaks.insert(self.input_flow_id.clone(),
                     get_track_peak_meters(self.input_track, self.spec.input_channels));

        peaks.insert(self.output_flow_id.clone(),
                     get_track_peak_meters(self.input_track, self.spec.input_channels));
    }

    pub fn set_master_send(&mut self, master_send: bool) {
        set_track_master_send(self.output_track, master_send);
    }

    pub fn prepare_render(&mut self, render: &RenderSession) {
        let reaper = Reaper::get();

        unsafe {
            // arm for recording
            reaper.get_set_media_track_info(self.output_track, TrackAttributeKey::RecArm, &mut 1i32 as *mut i32 as _);

            // set record mode to "output latency compensated"
            reaper.get_set_media_track_info(self.output_track,
                                            TrackAttributeKey::RecMode,
                                            &mut 3i32 as *mut i32 as _);

            // set record monitoring to off
            reaper.get_set_media_track_info(self.output_track, TrackAttributeKey::RecMon, &mut 0i32 as *mut i32 as _);
        }
    }

    pub fn clear_render(&mut self) -> Option<PathBuf> {
        let reaper = Reaper::get();
        let mut rv = None;

        // iterate all media items
        unsafe {
            while let Some(media_item) = reaper.get_track_media_item(self.output_track, 0) {
                if let Some(take) = reaper.get_active_take(media_item) {
                    if let Some(source) = reaper.get_media_item_take_source(take) {
                        let mut path_name = [0i8; 1024];

                        reaper.low()
                              .GetMediaSourceFileName(source.as_ptr(), path_name.as_mut_ptr(), path_name.len() as i32);

                        rv = Some(PathBuf::from(CStr::from_ptr(path_name.as_ptr()).to_string_lossy().to_string()));
                    }
                }

                if let Err(err) = reaper.delete_track_media_item(self.output_track, media_item) {
                    warn!(%err, "failed to delete media item");
                }
            }
        }

        rv
    }
}

#[derive(Template)]
#[template(path = "audio_engine/mixer_track_input.txt")]
struct AudioEngineMixerInputTemplate<'a> {
    project: &'a AudioEngineProjectTemplateSnapshot,
    mixer:   &'a AudioEngineMixer,
}

#[derive(Template)]
#[template(path = "audio_engine/mixer_track_output.txt")]
struct AudioEngineMixerOutputTemplate<'a> {
    project: &'a AudioEngineProjectTemplateSnapshot,
    mixer:   &'a AudioEngineMixer,
}
