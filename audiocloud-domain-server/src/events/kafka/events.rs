use actix::{Actor, ActorContext, Addr, Context, Handler, Supervised, Supervisor};
use actix_broker::BrokerSubscribe;
use anyhow::anyhow;
use once_cell::sync::OnceCell;
use rdkafka::config::FromClientConfigAndContext;
use rdkafka::producer::{BaseProducer, BaseRecord, DefaultProducerContext};
use sqlx::types::bstr::ByteSlice;
use tracing::*;

use audiocloud_api::{Codec, Json};

use crate::events::messages::NotifyDomainEvent;

static KAFKA_DOMAIN_EVENTS_SINK: OnceCell<Addr<KafkaDomainEventsSink>> = OnceCell::new();

pub async fn init(topic: String, brokers: String, username: String, password: String) -> anyhow::Result<()> {
    KAFKA_DOMAIN_EVENTS_SINK.set(Supervisor::start(move |_| KafkaDomainEventsSink { topic,
                                                                                    brokers,
                                                                                    username,
                                                                                    password,
                                                                                    producer: None }))
                            .map_err(|_| anyhow!("KAFKA_DOMAIN_EVENTS_SINK already initialized"))?;

    Ok(())
}

pub struct KafkaDomainEventsSink {
    topic:    String,
    brokers:  String,
    username: String,
    password: String,
    producer: Option<BaseProducer>,
}

impl Actor for KafkaDomainEventsSink {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.restarting(ctx);
    }
}

impl Supervised for KafkaDomainEventsSink {
    fn restarting(&mut self, ctx: &mut <Self as Actor>::Context) {
        self.subscribe_system_async::<NotifyDomainEvent>(ctx);

        let config = super::create_config(&self.brokers, &self.username, &self.password);

        self.producer =
            Some(BaseProducer::from_config_and_context(&config, DefaultProducerContext).expect("create producer"));
    }
}

impl Handler<NotifyDomainEvent> for KafkaDomainEventsSink {
    type Result = ();

    fn handle(&mut self, msg: NotifyDomainEvent, ctx: &mut Self::Context) -> Self::Result {
        match self.producer.as_mut() {
            Some(producer) => match Json.serialize(&msg.event) {
                Ok(encoded) => {
                    let key = msg.event.key();
                    if let Err(error) = producer.send(BaseRecord::to(&self.topic).key(&key).payload(encoded.as_bytes()))
                    {
                        warn!(?error, "Failed to send domain event to Kafka")
                    }
                }
                Err(error) => {
                    warn!(?error, "Failed to serialize event");
                }
            },
            None => {
                error!("Kafka producer not initialized");
                ctx.stop();
            }
        }
    }
}
