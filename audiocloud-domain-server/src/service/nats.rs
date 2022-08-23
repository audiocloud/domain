use actix::{spawn, SystemService};
use clap::Args;
use itertools::Itertools;
use nats_aflowt::Connection;
use once_cell::sync::OnceCell;
use tracing::*;

use audiocloud_api::audio_engine::{AudioEngineCommand, AudioEngineEvent};
use audiocloud_api::codec::{Codec, Json, MsgPack};
use audiocloud_api::driver::{InstanceDriverCommand, InstanceDriverError, InstanceDriverEvent};
use audiocloud_api::error::SerializableResult;
use audiocloud_api::media::MediaServiceEvent;
use audiocloud_api::newtypes::{AudioEngineId, FixedInstanceId, MediaServiceId, ModelId};

use crate::service::instance::{InstancesSupervisor, NotifyInstanceDriver};
use crate::service::session::messages::{NotifyAudioEngineEvent, NotifyMediaServiceEvent};
use crate::service::session::supervisor::SessionsSupervisor;

#[derive(Args)]
pub struct NatsOpts {
    #[clap(long, short, env, default_value = "nats://localhost:4222")]
    pub nats_url: String,
}

static NATS_CLIENT: OnceCell<NatsClient> = OnceCell::new();

pub fn get_nats_client() -> &'static NatsClient {
    NATS_CLIENT.get().expect("NATS client not initialized")
}

pub struct NatsClient {
    connection: Connection,
}

impl NatsClient {
    pub async fn request_audio_engine(&self,
                                      engine_id: &AudioEngineId,
                                      request: AudioEngineCommand)
                                      -> anyhow::Result<()> {
        let encoded = MsgPack.serialize(&request)?;
        let topic = format!("ac.aeng.{}.cmd", engine_id);
        let response = self.connection.request(&topic, encoded).await?;
        MsgPack.deserialize::<SerializableResult>(&response.data)?.into()
    }

    pub async fn request_instance_driver(&self,
                                         fixed_instance_id: &FixedInstanceId,
                                         request: InstanceDriverCommand)
                                         -> anyhow::Result<()> {
        let encoded = Json.serialize(&request)?;
        let topic = format!("ac.inst.{}.{}.{}.cmd",
                            &fixed_instance_id.manufacturer, &fixed_instance_id.name, &fixed_instance_id.instance);

        let response = self.connection.request(&topic, encoded).await?;

        Ok(Json.deserialize::<Result<(), InstanceDriverError>>(&response.data)??)
    }
}

pub async fn init(opts: NatsOpts) -> anyhow::Result<()> {
    let client = nats_aflowt::connect(&opts.nats_url).await?;
    let inst_evt = client.subscribe("ac.inst.*.*.*.evt").await?;
    let aeng_evt = client.subscribe("ac.aeng.*.evt").await?;
    let mdia_evt = client.subscribe("ac.mdia.*.evt").await?;

    spawn(async move {
        while let Some(msg) = inst_evt.next().await {
            match msg.subject
                     .split('.')
                     .skip(2)
                     .take(3)
                     .collect_tuple::<(&str, &str, &str)>()
            {
                Some((manufacturer, name, instance)) => {
                    let instance_id =
                        ModelId::new(manufacturer.to_owned(), name.to_owned()).instance(instance.to_owned());
                    match Json.deserialize::<InstanceDriverEvent>(&msg.data[..]) {
                        Ok(event) => {
                            InstancesSupervisor::from_registry().do_send(NotifyInstanceDriver { instance_id, event });
                        }
                        Err(err) => {
                            error!(%err, "Failed to deserialize instance driver event");
                        }
                    }
                }
                None => {
                    warn!(subject = %msg.subject, "Invalid subject");
                }
            }
        }
    });

    spawn(async move {
        while let Some(msg) = aeng_evt.next().await {
            match msg.subject.split('.').nth(2) {
                Some(engine_id) => {
                    let engine_id = AudioEngineId::new(engine_id.to_owned());
                    match MsgPack.deserialize::<AudioEngineEvent>(&msg.data[..]) {
                        Ok(event) => {
                            SessionsSupervisor::from_registry().do_send(NotifyAudioEngineEvent { engine_id, event });
                        }
                        Err(err) => {
                            error!(%err, "Failed to deserialize audio engine event");
                        }
                    }
                }
                None => {
                    warn!(subject = %msg.subject, "Invalid subject");
                }
            }
        }
    });

    spawn(async move {
        while let Some(msg) = mdia_evt.next().await {
            match msg.subject.split('.').nth(2) {
                Some(media_service_id) => {
                    let media_service_id = MediaServiceId::new(media_service_id.to_owned());
                    match MsgPack.deserialize::<MediaServiceEvent>(&msg.data[..]) {
                        Ok(event) => {
                            SessionsSupervisor::from_registry().do_send(NotifyMediaServiceEvent { media_service_id,
                                                                                                  event });
                        }
                        Err(err) => {
                            error!(%err, "Failed to deserialize audio engine event");
                        }
                    }
                }
                None => {
                    warn!(subject = %msg.subject, "Invalid subject");
                }
            }
        }
    });

    Ok(())
}
