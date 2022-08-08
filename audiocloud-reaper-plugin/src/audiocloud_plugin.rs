use std::cell::Cell;
use std::ffi::CString;
use std::str::FromStr;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;
use std::time::Duration;
use std::{env, thread};

use reaper_low::{static_vst_plugin_context, PluginContext};
use reaper_medium::{ProjectRef, Reaper, ReaperSession};
use tracing::*;
use vst::prelude::*;

use audiocloud_api::audio_engine::AudioEngineEvent;
use audiocloud_api::codec::{Codec, MsgPack};
use audiocloud_api::newtypes::AppSessionId;

use crate::audio_engine::ReaperAudioEngine;
use crate::events::AudioEngineCommandWithResultSender;

pub struct AudioCloudPlugin {
    id:        AppSessionId,
    rx_plugin: flume::Receiver<()>,
    tx_engine: flume::Sender<()>,
}

const FIRST_TIME_BOOT: AtomicBool = AtomicBool::new(false);
const SESSION_WRAPPER: Cell<Option<ReaperSession>> = Cell::new(None);

impl Plugin for AudioCloudPlugin {
    fn get_info(&self) -> Info {
        Info { name: "Audiocloud Plugin".to_string(),
               unique_id: 0xbad1337,
               f64_precision: true,
               ..Default::default() }
    }

    fn new(host: HostCallback) -> Self
        where Self: Sized
    {
        if FIRST_TIME_BOOT.compare_exchange(false, true, SeqCst, SeqCst).is_ok() {
            init_env();
            let ctx = PluginContext::from_vst_plugin(&host, static_vst_plugin_context()).expect("REAPER PluginContext init success");
            let mut session = ReaperSession::load(ctx);
            let reaper = session.reaper().clone();

            Reaper::make_available_globally(reaper);

            let (tx_cmd, rx_cmd) = flume::unbounded::<AudioEngineCommandWithResultSender>();
            let (tx_evt, rx_evt) = flume::unbounded::<AudioEngineEvent>();

            let nats_url = env::var("NATS_URL").expect("NATS_URL env var must be set");
            let subscribe_topic = env::var("NATS_CMD_TOPIC").expect("NATS_CMD_TOPIC env var must be set");
            let publish_topic = env::var("NATS_EVT_TOPIC").expect("NATS_EVT_TOPIC env var must be set");

            let connection = nats::connect(nats_url).expect("NATS connection success");
            let subscription = connection.subscribe(&subscribe_topic)
                                         .expect("NATS subscription success");

            thread::spawn(move || {
                while let Some(msg) = subscription.next() {
                    if let Ok(cmd) = MsgPack.deserialize(&msg.data[..]) {
                        let (tx, rx) = flume::unbounded::<anyhow::Result<()>>();
                        if let Ok(_) = tx_cmd.send((cmd, tx)) {
                            thread::spawn(move || {
                                let result = match rx.recv_timeout(Duration::from_millis(500)) {
                                    Err(_) => Err(format!("Request timed out")),
                                    Ok(Err(err)) => Err(err.to_string()),
                                    Ok(Ok(result)) => Ok(result),
                                };

                                let result = MsgPack.serialize(&result).expect("Response serialization success");
                                msg.respond(result).expect("NATS response send");
                            });
                        }
                    }
                }
            });

            thread::spawn(move || {
                while let Ok(evt) = rx_evt.recv() {
                    if let Ok(encoded) = MsgPack.serialize(&evt) {
                        if let Err(err) = connection.publish(&publish_topic, encoded) {
                            warn!(%err, "failed to publish event");
                        }
                    }
                }
            });

            session.plugin_register_add_csurf_inst(Box::new(ReaperAudioEngine::new(rx_cmd, tx_evt)))
                   .expect("REAPER audio engine control surface register success");

            reaper.show_console_msg("--- Audiocloud Plugin first boot ---\n");

            SESSION_WRAPPER.set(Some(session));
        }

        let reaper = Reaper::get();
        let project = reaper.enum_projects(ProjectRef::Current, 0)
                            .expect("REAPER project enum success")
                            .project;

        let id = unsafe {
            let mut notes = [0i8; 1024];
            reaper.low()
                  .GetSetProjectNotes(project.as_ptr(), false, notes.as_mut_ptr(), notes.len() as i32);
            let cstr = CString::from_raw(notes.as_mut_ptr());

            AppSessionId::from_str(cstr.to_string_lossy().as_ref()).expect("AppSessionId parse success")
        };

        reaper.show_console_msg("Streaming plugin initializing\n");

        let (tx_plugin, rx_plugin) = flume::unbounded::<()>();
        let (tx_engine, rx_engine) = flume::unbounded::<()>();

        Self { id,
               rx_plugin,
               tx_engine }
    }

    fn set_sample_rate(&mut self, rate: f32) {}

    fn resume(&mut self) {}

    fn suspend(&mut self) {}

    fn process_f64(&mut self, buffer: &mut AudioBuffer<f64>) {
        // TODO: stuff...
    }
}

impl Drop for AudioCloudPlugin {
    fn drop(&mut self) {
        if let Some(comm) = &self.comm {
            ReaperAudioEngine::deregister_plugin(&comm.id);
        }
        Reaper::get().show_console_msg("Streaming plugin dropping\n");
    }
}

fn init_env() {
    let _ = dotenv::dotenv();
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "info,audiocloud_reaper_plugin=debug,audiocloud_api=debug");
    }

    tracing_subscriber::fmt::init();
}
