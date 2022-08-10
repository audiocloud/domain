use std::ffi::CStr;
use std::str::FromStr;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use std::{env, thread};

use once_cell::sync::OnceCell;
use reaper_low::{static_vst_plugin_context, PluginContext};
use reaper_medium::{ProjectRef, Reaper, ReaperSession};
use tracing::*;
use vst::prelude::*;

use audiocloud_api::audio_engine::AudioEngineEvent;
use audiocloud_api::codec::{Codec, MsgPack};
use audiocloud_api::newtypes::AppSessionId;

use crate::audio_engine::{PluginRegistry, ReaperAudioEngine, ReaperEngineCommand, StreamingPluginCommand};

pub struct AudioCloudPlugin {
    activation:         Option<AudioCloudPluginActivation>,
    native_sample_rate: usize,
    host:               HostCallback,
}

struct AudioCloudPluginActivation {
    id:        AppSessionId,
    rx_plugin: flume::Receiver<StreamingPluginCommand>,
    tx_engine: flume::Sender<ReaperEngineCommand>,
}

const FIRST_TIME_BOOT: AtomicBool = AtomicBool::new(false);
const SESSION_WRAPPER: OnceCell<ReaperSession> = OnceCell::new();

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
        SESSION_WRAPPER.get_or_init(|| {
                           eprintln!("==== first time boot ====");
                           init_env();
                           init_audio_engine(&host)
                       });

        Self { activation: None,
               native_sample_rate: 192_000,
               host }
    }

    fn init(&mut self) {
        let reaper = Reaper::get();
        let project = reaper.enum_projects(ProjectRef::Current, 0)
                            .expect("REAPER project enum success")
                            .project;

        let maybe_id = unsafe {
            let mut notes = [0i8; 1024];

            reaper.low()
                  .GetSetProjectNotes(project.as_ptr(), false, notes.as_mut_ptr(), notes.len() as i32);

            let cstr = CStr::from_ptr(notes.as_ptr());

            AppSessionId::from_str(cstr.to_string_lossy().as_ref())
        };

        self.activation = maybe_id.ok().map(|id| {
                                           let (tx_plugin, rx_plugin) = flume::unbounded();
                                           let tx_engine = PluginRegistry::register(id.clone(), tx_plugin);

                                           AudioCloudPluginActivation { id,
                                                                        rx_plugin,
                                                                        tx_engine }
                                       });
    }

    fn set_sample_rate(&mut self, rate: f32) {
        self.native_sample_rate = rate as usize;
    }

    fn resume(&mut self) {
        debug!("resume");
    }

    fn suspend(&mut self) {
        debug!("suspend");
    }

    fn process_f64(&mut self, buffer: &mut AudioBuffer<f64>) {
        trace!(samples = buffer.samples(),
               inputs = buffer.input_count(),
               outputs = buffer.output_count(),
               "process_f64");
    }
}

impl Drop for AudioCloudPlugin {
    fn drop(&mut self) {
        if let Some(activation) = &self.activation {
            PluginRegistry::unregister(&activation.id);
        }
    }
}

fn init_env() {
    let _ = dotenv::dotenv();
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "info,audiocloud_reaper_plugin=debug,audiocloud_api=debug");
    }

    tracing_subscriber::fmt::init();
}

fn init_audio_engine(host: &HostCallback) -> ReaperSession {
    let ctx =
        PluginContext::from_vst_plugin(host, static_vst_plugin_context()).expect("REAPER PluginContext init success");
    let mut session = ReaperSession::load(ctx);
    let reaper = session.reaper().clone();

    Reaper::make_available_globally(reaper);

    let (tx_cmd, rx_cmd) = flume::unbounded();
    let (tx_evt, rx_evt) = flume::unbounded::<AudioEngineEvent>();

    let nats_url = env::var("NATS_URL").expect("NATS_URL env var must be set");
    let subscribe_topic = env::var("NATS_CMD_TOPIC").expect("NATS_CMD_TOPIC env var must be set");
    let publish_topic = env::var("NATS_EVT_TOPIC").expect("NATS_EVT_TOPIC env var must be set");

    let connection = nats::connect(nats_url).expect("NATS connection success");
    let subscription = connection.subscribe(&subscribe_topic)
                                 .expect("NATS subscription success");

    thread::spawn({
        let tx_cmd = tx_cmd.clone();
        move || {
            while let Some(msg) = subscription.next() {
                if let Ok(cmd) = MsgPack.deserialize(&msg.data[..]) {
                    let (tx, rx) = flume::unbounded::<anyhow::Result<()>>();
                    if let Ok(_) = tx_cmd.send(ReaperEngineCommand::Request((cmd, tx))) {
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

    PluginRegistry::init(tx_cmd.clone());

    session.plugin_register_add_csurf_inst(Box::new(ReaperAudioEngine::new(tx_cmd.clone(), rx_cmd, tx_evt)))
           .expect("REAPER audio engine control surface register success");

    info!("init complete");

    session
}
