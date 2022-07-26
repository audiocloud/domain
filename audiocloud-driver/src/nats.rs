use std::collections::HashSet;

use actix::{Actor, AsyncContext, Context, Handler, Message, Supervised, SystemService};
use actix_broker::BrokerSubscribe;
use anyhow::anyhow;
use clap::Args;
use nats_aflowt::Connection;
use once_cell::sync::OnceCell;
use serde::Serialize;
use tokio::spawn;

use audiocloud_api::codec::{Codec, Json};
use audiocloud_api::driver::InstanceDriverCommand;
use audiocloud_api::newtypes::FixedInstanceId;

use crate::Event;

#[derive(Args, Clone, Debug)]
pub struct NatsOpts {
    #[clap(env, long, default_value = "nats://localhost:4222")]
    pub nats_url: String,
}

static NATS: OnceCell<Connection> = OnceCell::new();

pub async fn init(opts: NatsOpts, instances: HashSet<FixedInstanceId>) -> anyhow::Result<()> {
    let connection = nats_aflowt::connect(opts.nats_url.as_str()).await?;

    for instance_id in instances {
        let manufacturer = &instance_id.manufacturer;
        let model = &instance_id.name;
        let instance = &instance_id.instance;
        let subscription = connection.subscribe(&format!("ac.insts.{manufacturer}.{model}.{instance}.cmds"))
            .await?;

        spawn(handle_commands(subscription, instance_id));
    }

    NATS.set(connection)
        .map_err(|_| anyhow!("State init already called!"))?;

    Ok(())
}

pub fn get_nats() -> &'static Connection {
    NATS.get().expect("NATS not initialized")
}

async fn handle_commands(subscription: nats_aflowt::Subscription, instance_id: FixedInstanceId) {
    while let Some(msg) = subscription.next().await {
        if let Ok(cmd) = Json.deserialize::<InstanceDriverCommand>(&msg.data) {}
    }
}

#[derive(Default)]
pub struct NatsService;

impl Actor for NatsService {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.restarting(ctx);
    }
}

impl Supervised for NatsService {
    fn restarting(&mut self, ctx: &mut Self::Context) {
        self.subscribe_system_async::<Event>(ctx);
    }
}

impl SystemService for NatsService {}

impl<S: Serialize + Send, C: Codec + Send> Handler<Publish<S, C>> for NatsService {
    type Result = ();

    fn handle(&mut self, msg: Publish<S, C>, _ctx: &mut Self::Context) -> Self::Result {
        if let Ok(serialized) = msg.codec.serialize(&msg.message) {
            spawn(async move {
                let _ = get_nats().publish(&msg.subject, &serialized).await;
            });
        }
    }
}

impl Handler<Event> for NatsService {
    type Result = ();

    fn handle(&mut self, msg: Event, ctx: &mut Self::Context) -> Self::Result {
        let id = &msg.instance_id;
        ctx.notify(Publish { subject: format!("ac.insts.{}.{}.{}.evts", id.manufacturer, id.name, id.instance),
                             message: msg.event,
                             codec:   Json, });
    }
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct Publish<S: Serialize, C: Codec> {
    pub subject: String,
    pub message: S,
    pub codec:   C,
}
