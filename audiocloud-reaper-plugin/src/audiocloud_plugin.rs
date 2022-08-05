use std::cell::Cell;
use std::env;
use std::str::FromStr;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;

use reaper_low::{PluginContext, static_vst_plugin_context};
use reaper_medium::{Reaper, ReaperSession};
use tracing::*;
use vst::prelude::*;

use audiocloud_api::audio_engine::{AudioEngineCommand, AudioEngineEvent, CompressedAudio};
use audiocloud_api::newtypes::AppSessionId;

use crate::control_surface::AudiocloudControlSurface;
use crate::events::{ControlSurfaceCommand, ControlSurfaceCommandWithResultSender, ControlSurfaceEvent};

pub struct AudiocloudPlugin {}

const FIRST_TIME_BOOT: AtomicBool = AtomicBool::new(false);
const SESSION_WRAPPER: Cell<Option<ReaperSession>> = Cell::new(None);

impl Plugin for AudiocloudPlugin {
    fn get_info(&self) -> Info {
        Info {
            name: "Audiocloud Plugin".to_string(),
            unique_id: 0xbad1337,
            f64_precision: true,
            ..Default::default()
        }
    }

    fn new(host: HostCallback) -> Self
        where Self: Sized
    {
        if FIRST_TIME_BOOT.compare_exchange(false, true, SeqCst, SeqCst).is_ok() {
            init_env();
            debug!("Audiocloud Plugin: First time boot");
        } else {
            debug!("Audiocloud Plugin: Subsequent boot");
        }

        let ctx = PluginContext::from_vst_plugin(&host, static_vst_plugin_context()).expect("REAPER PluginContext init success");
        let mut session = ReaperSession::load(ctx);
        let reaper = session.reaper().clone();

        Reaper::make_available_globally(reaper);

        let (tx_cmd, rx_cmd) = flume::unbounded::<ControlSurfaceCommandWithResultSender>();
        let (tx_evt, rx_evt) = flume::unbounded::<ControlSurfaceEvent>();
        let (tx_aud, rx_aud) = flume::unbounded::<CompressedAudio>();

        let session_id = env::var("SESSION_ID").expect("SESSION_ID is set");
        let spec = env::var("SESSION_SPEC").expect("SESSION_SPEC is set");

        let session_id = AppSessionId::from_str(&session_id).expect("SESSION_ID is valid");
        let spec = serde_json::from_str(&spec).expect("SESSION_SPEC is valid JSON");

        session.plugin_register_add_csurf_inst(Box::new(AudiocloudControlSurface::new(session_id, spec, rx_cmd, tx_evt))).expect("REAPER plugin register success");

        SESSION_WRAPPER.set(Some(session));

        Self {}
    }
}

impl Drop for AudiocloudPlugin {
    fn drop(&mut self) {
        SESSION_WRAPPER.set(None);
    }
}

fn init_env() {
    let _ = dotenv::dotenv();
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "info,audiocloud_reaper_plugin=debug,audiocloud_api=debug");
    }

    tracing_subscriber::fmt::init();
}
