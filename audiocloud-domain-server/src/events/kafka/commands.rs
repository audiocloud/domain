use actix::{Actor, ActorContext, Addr, AsyncContext, Context, StreamHandler, Supervised, Supervisor};
use actix_broker::BrokerIssue;
use anyhow::anyhow;
use futures::StreamExt;
use once_cell::sync::OnceCell;
use rdkafka::config::FromClientConfigAndContext;
use rdkafka::consumer::{Consumer, ConsumerContext, Rebalance, StreamConsumer};
use rdkafka::error::KafkaResult;
use rdkafka::message::OwnedMessage;
use rdkafka::{ClientContext, Message, Offset, TopicPartitionList};
use tracing::*;

use audiocloud_api::{Codec, Json};

use crate::events::messages::NotifyDomainSessionCommand;

static KAFKA_DOMAIN_COMMANDS_LISTENER: OnceCell<Addr<KafkaDomainCommandsListener>> = OnceCell::new();

pub async fn init(topic: String,
                  brokers: String,
                  username: String,
                  password: String,
                  maybe_offset: Option<i64>)
                  -> anyhow::Result<()> {
    KAFKA_DOMAIN_COMMANDS_LISTENER.set(Supervisor::start(move || KafkaDomainCommandsListener { topic,
                                                                                               brokers,
                                                                                               username,
                                                                                               password,
                                                                                               maybe_offset }))
                                  .map_err(|_| anyhow!("KAFKA_DOMAIN_COMMANDS_LISTENER already initialized"))?;
    Ok(())
}

struct CustomContext;
type LoggingConsumer = StreamConsumer<CustomContext>;

impl ClientContext for CustomContext {}

impl ConsumerContext for CustomContext {
    fn pre_rebalance(&self, rebalance: &Rebalance) {
        info!(?rebalance, "Pre rebalance");
    }

    fn post_rebalance(&self, rebalance: &Rebalance) {
        info!(?rebalance, "Post rebalance");
    }

    fn commit_callback(&self, result: KafkaResult<()>, _offsets: &TopicPartitionList) {
        info!(?result, "Committing offsets:");
    }
}

struct KafkaDomainCommandsListener {
    topic:        String,
    brokers:      String,
    username:     String,
    password:     String,
    maybe_offset: Option<i64>,
}

impl Actor for KafkaDomainCommandsListener {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.restarting(ctx);
    }
}

impl Supervised for KafkaDomainCommandsListener {
    fn restarting(&mut self, ctx: &mut <Self as Actor>::Context) {
        let config = super::create_config(&self.brokers, &self.username, &self.password);

        let consumer = LoggingConsumer::from_config_and_context(&config, CustomContext)?;

        let mut topics = TopicPartitionList::new();
        topics.add_partition_offset(&self.topic, 0, match self.maybe_offset {
                  None => Offset::OffsetTail(100),
                  Some(offset) => Offset::Offset(offset as i64),
              })?;

        consumer.assign(&topics)?;

        ctx.add_stream(consumer.stream());
    }
}

impl StreamHandler<KafkaResult<OwnedMessage>> for KafkaDomainCommandsListener {
    fn handle(&mut self, item: KafkaResult<OwnedMessage>, ctx: &mut Self::Context) {
        match item {
            Ok(item) => {
                let offset = item.offset();
                self.maybe_offset = Some(offset);

                match item.payload() {
                    Some(payload) => match Json.deserialize(payload) {
                        Ok(command) => {
                            self.issue_system_async(NotifyDomainSessionCommand { command });
                        }
                        Err(error) => {
                            warn!(?error, %offset, "Failed to deserialize command");
                        }
                    },
                    None => {
                        warn!(%offset, "No message content");
                    }
                }
            }
            Err(error) => {
                warn!(?error, "Kafka consumer error");
                ctx.stop();
            }
        }
    }
}