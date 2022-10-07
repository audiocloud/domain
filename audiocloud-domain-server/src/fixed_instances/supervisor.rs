use std::collections::HashMap;

use actix::fut::LocalBoxActorFuture;
use actix::{
    fut, Actor, ActorFutureExt, Addr, Context, ContextFutureSpawner, Handler, MessageResult, Supervised, Supervisor,
    WrapFuture,
};
use actix_broker::BrokerSubscribe;
use anyhow::anyhow;
use futures::executor::block_on;
use tracing::warn;

use audiocloud_api::cloud::domains::{
    DomainConfig, DomainFixedInstanceConfig, FixedInstanceRouting, FixedInstanceRoutingMap,
};
use audiocloud_api::domain::DomainError;
use audiocloud_api::{hashmap_changes, FixedInstanceId, HashMapChanges};

use crate::config::NotifyDomainConfiguration;
use crate::db::Db;
use crate::fixed_instances::instance::InstanceActor;
use crate::fixed_instances::{
    GetMultipleFixedInstanceState, NotifyInstancePowerChannelsChanged, NotifyInstanceState, SetDesiredPowerChannel,
    SetInstanceDesiredPlayState, SetInstanceParameters,
};
use crate::DomainResult;

pub struct FixedInstancesSupervisor {
    instances: HashMap<FixedInstanceId, Instance>,
    db:        Db,
}

struct Instance {
    address: Addr<InstanceActor>,
    config:  DomainFixedInstanceConfig,
    state:   Option<NotifyInstanceState>,
}

impl FixedInstancesSupervisor {
    pub async fn new(boot: &DomainConfig, db: Db) -> anyhow::Result<(FixedInstanceRoutingMap, Self)> {
        let mut routing = HashMap::new();
        let mut instances = HashMap::new();

        for (id, config) in &boot.fixed_instances {
            let model = db.get_model(&id.model_id())
                          .await?
                          .ok_or_else(|| anyhow!("Missing model for instance {id}"))?;

            let send_count = model.inputs.len();
            let return_count = model.outputs.len();

            let actor = InstanceActor::new(id.clone(), config.clone(), model)?;

            if let (Some(input_start), Some(output_start)) = (config.input_start, config.output_start) {
                routing.insert(id.clone(),
                               FixedInstanceRouting { send_count:     { send_count },
                                                      send_channel:   { output_start as usize },
                                                      return_count:   { return_count },
                                                      return_channel: { input_start as usize }, });
            }

            instances.insert(id.clone(),
                             Instance { address: { Supervisor::start(move |_| actor) },
                                        config:  { config.clone() },
                                        state:   None, });
        }

        Ok((routing, Self { db, instances }))
    }
}

impl Actor for FixedInstancesSupervisor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.restarting(ctx);
    }
}

impl Supervised for FixedInstancesSupervisor {
    fn restarting(&mut self, ctx: &mut Self::Context) {
        self.subscribe_system_async::<NotifyDomainConfiguration>(ctx);
        self.subscribe_system_async::<NotifyInstancePowerChannelsChanged>(ctx);
    }
}

impl Handler<NotifyDomainConfiguration> for FixedInstancesSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyDomainConfiguration, ctx: &mut Self::Context) -> Self::Result {
        let existing = self.instances
                           .iter()
                           .map(|(id, instance)| (id.clone(), instance.config.clone()))
                           .collect();

        let HashMapChanges { changed,
                             added,
                             removed, } = hashmap_changes(&existing, &msg.config.fixed_instances);

        for id in removed {
            self.instances.remove(&id);
        }

        for (id, config) in added {
            if let Ok(Some(model)) = block_on(self.db.get_model(&id.model_id())) {
                match InstanceActor::new(id.clone(), config.clone(), model) {
                    Ok(actor) => {
                        let address = Supervisor::start(move |_| actor);

                        self.instances.insert(id.clone(),
                                              Instance { address: { address },
                                                         config:  { config },
                                                         state:   { None }, });
                    }
                    Err(error) => {
                        warn!(%id, %error, "Could not create instance actor");
                    }
                }
            }
        }

        for (id, config) in changed {
            if let Some(instance) = self.instances.get_mut(&id) {
                instance.config = config;
                // TODO: set configuration of instance actor
            }
        }
    }
}

impl Handler<SetInstanceParameters> for FixedInstancesSupervisor {
    type Result = LocalBoxActorFuture<Self, DomainResult>;

    fn handle(&mut self, msg: SetInstanceParameters, ctx: &mut Context<FixedInstancesSupervisor>) -> Self::Result {
        if let Some(instance) = self.instances.get(&msg.instance_id) {
            instance.address
                    .send(msg)
                    .into_actor(self)
                    .map(|res, actor, ctx| match res {
                        Ok(res) => res,
                        Err(err) => {
                            Err(DomainError::BadGateway { error: format!("Failed to set instance parameters: {err}"), })
                        }
                    })
                    .boxed_local()
        } else {
            fut::err(DomainError::InstanceNotFound { instance_id: msg.instance_id, }).boxed_local()
        }
    }
}

impl Handler<SetDesiredPowerChannel> for FixedInstancesSupervisor {
    type Result = LocalBoxActorFuture<Self, DomainResult>;

    fn handle(&mut self, msg: SetDesiredPowerChannel, ctx: &mut Self::Context) -> Self::Result {
        if let Some(instance) = self.instances.get(&msg.instance_id) {
            instance.address
                    .send(msg)
                    .into_actor(self)
                    .map(|res, actor, ctx| match res {
                        Ok(res) => res,
                        Err(err) => {
                            Err(DomainError::BadGateway { error: format!("Failed to set instance power: {err}"), })
                        }
                    })
                    .boxed_local()
        } else {
            fut::err(DomainError::InstanceNotFound { instance_id: msg.instance_id, }).boxed_local()
        }
    }
}

impl Handler<SetInstanceDesiredPlayState> for FixedInstancesSupervisor {
    type Result = LocalBoxActorFuture<Self, DomainResult>;

    fn handle(&mut self, msg: SetInstanceDesiredPlayState, ctx: &mut Self::Context) -> Self::Result {
        if let Some(instance) = self.instances.get(&msg.instance_id) {
            instance.address
        .send(msg)
        .into_actor(self)
        .map(|res, actor, ctx| match res {
          Ok(res) => res,
          Err(err) => Err(DomainError::BadGateway { error: format!("Failed to set instance desired play state: {err}") }),
        })
        .boxed_local()
        } else {
            fut::err(DomainError::InstanceNotFound { instance_id: msg.instance_id, }).boxed_local()
        }
    }
}

impl Handler<GetMultipleFixedInstanceState> for FixedInstancesSupervisor {
    type Result = MessageResult<GetMultipleFixedInstanceState>;

    fn handle(&mut self, msg: GetMultipleFixedInstanceState, ctx: &mut Self::Context) -> Self::Result {
        let mut rv = HashMap::new();

        for id in msg.instance_ids {
            if let Some(instance) = self.instances.get(&id) {
                if let Some(state) = instance.state.clone() {
                    rv.insert(id.clone(), state);
                }
            }
        }

        MessageResult(rv)
    }
}

impl Handler<NotifyInstancePowerChannelsChanged> for FixedInstancesSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyInstancePowerChannelsChanged, ctx: &mut Self::Context) -> Self::Result {
        // inform all connected instances
        for instance in self.instances.values() {
            if let Some(power_config) = &instance.config.power {
                if &power_config.instance == &msg.instance_id {
                    instance.address.do_send(msg.clone());
                }
            }
        }
    }
}
