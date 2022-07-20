use actix::spawn;
use anyhow::anyhow;
use clap::Args;
use futures::StreamExt;
use once_cell::sync::OnceCell;
use rdkafka::config::{FromClientConfigAndContext, RDKafkaLogLevel};
use rdkafka::consumer::stream_consumer::StreamConsumer;
use rdkafka::consumer::{Consumer, ConsumerContext, Rebalance};
use rdkafka::error::KafkaResult;
use rdkafka::message::BorrowedMessage;
use rdkafka::{ClientConfig, Message, TopicPartitionList};
use rdkafka::{ClientContext, Offset};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::Url;
use tracing::*;

use audiocloud_api::cloud::domains::BootDomain;
use audiocloud_api::domain::DomainSessionCommand;

#[derive(Args, Debug)]
pub struct CloudOpts {
    #[clap(short = 'k', long, env)]
    api_key: String,

    #[clap(long, env, default_value = "https://api.audiocloud.io")]
    api_url: Url,
}

type LoggingConsumer = StreamConsumer<CustomContext>;

struct CloudClient {
    client:   reqwest::Client,
    consumer: LoggingConsumer,
}

static CLOUD_CLIENT: OnceCell<CloudClient> = OnceCell::new();

fn get_cloud_client() -> &'static CloudClient {
    CLOUD_CLIENT.get().expect("Cloud client must be initialized")
}

struct CustomContext;

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

#[instrument(skip(opts), err)]
pub async fn init(opts: CloudOpts) -> anyhow::Result<BootDomain> {
    let mut headers = HeaderMap::new();
    headers.insert("X-Api-Key", HeaderValue::from_str(&opts.api_key)?);

    let client = reqwest::Client::builder().default_headers(headers)
                                           .tcp_nodelay(true)
                                           .build()?;

    let boot = client.get(opts.api_url.join("/v1/domains/boot")?)
                     .send()
                     .await?
                     .json::<BootDomain>()
                     .await?;

    let domain_id = &boot.domain_id;

    info!(%domain_id, "Booted domain!");

    let context = CustomContext;

    let mut config = ClientConfig::default();

    config.set("bootstrap.servers", boot.kafka_url.as_str())
          .set("security.protocol", "SASL_SSL")
          .set("sasl.mechanisms", "SCRAM-SHA-256")
          .set("sasl.username", boot.consume_username.as_str())
          .set("sasl.password", boot.consume_password.as_str())
          .set("session.timeout.ms", "6000")
          .set("enable.auto.commit", "true")
          .set("group.id", "audiocloud-domain-server")
          .set_log_level(RDKafkaLogLevel::Debug);

    debug!(kafka_url = boot.kafka_url,
           consume_username = boot.consume_username,
           consume_password = boot.consume_password,
           topic = &boot.cmd_topic,
           "group.id" = "audiocloud-domain-server",
           "Creating stream consumer");

    let consumer = LoggingConsumer::from_config_and_context(&config, context)?;

    let mut topics = TopicPartitionList::new();
    topics.add_partition_offset(&boot.cmd_topic, 0, Offset::OffsetTail(100))?;

    consumer.assign(&topics)?;

    CLOUD_CLIENT.set(CloudClient { client, consumer })
                .map_err(|_| anyhow!("Cloud client must only be called once"))?;

    Ok(boot)
}

pub async fn spawn_command_listener(event_base: i64) -> anyhow::Result<()> {
    let mut stream = get_cloud_client().consumer.stream();

    while let Some(message) = stream.next().await {
        let done = matches!(&message, Ok(msg) if msg.offset() >= (event_base - 1));
        dispatch_message(message).await;
        if done {
            info!("Caught up with cloud events");
            break;
        }
    }

    spawn(async move {
        while let Some(message) = stream.next().await {
            dispatch_message(message).await;
        }
    });

    Ok(())
}

async fn dispatch_message(msg: KafkaResult<BorrowedMessage<'_>>) {
    match msg {
        Ok(msg) => {
            info!(offset = msg.offset(), "--- --- message --- ---");
            match msg.payload_view::<str>() {
                None => {
                    warn!(?msg, "Message payload is not a string");
                }
                Some(payload) => match payload {
                    Ok(payload) => match serde_json::from_str::<DomainSessionCommand>(payload) {
                        Ok(command) => {
                            info!(?command, "Received command");
                        }
                        Err(err) => {
                            warn!(?err, "Failed to parse command");
                        }
                    },
                    Err(err) => {
                        warn!(%err, "Message payload is not a string");
                    }
                },
            }
        }
        Err(err) => {
            error!(%err, "Error consuming message");
        }
    }
}
