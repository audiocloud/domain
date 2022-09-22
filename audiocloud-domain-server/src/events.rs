use audiocloud_api::cloud::domains::{DomainCommandSource, DomainEventSink};

mod log_events;
mod noop_events;

pub mod events;
mod kafka;

pub async fn init(commands: DomainCommandSource, events: DomainEventSink) -> anyhow::Result<()> {
    match commands {
        DomainCommandSource::Disabled => {
            // nothing to do
        }
        DomainCommandSource::Kafka { topic,
                                     brokers,
                                     username,
                                     password,
                                     offset, } => {
            kafka::kafka_commands::init(topic, brokers, username, password, offset).await?;
        }
    }

    match events {
        DomainEventSink::Disabled => {
            noop_events::init().await?;
        }
        DomainEventSink::Log => {
            log_events::init().await?;
        }
        DomainEventSink::Kafka { topic,
                                 brokers,
                                 username,
                                 password, } => {
            kafka::kafka_events::init(topic, brokers, username, password).await?;
        }
    }

    Ok(())
}
