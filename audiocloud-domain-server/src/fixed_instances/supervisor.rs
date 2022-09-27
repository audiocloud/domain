use std::collections::HashMap;

use actix::{Actor, Addr, Context, Handler, Supervised, Supervisor};
use actix_broker::BrokerSubscribe;
use anyhow::anyhow;

use audiocloud_api::cloud::domains::{DomainConfig, DomainFixedInstanceConfig};
use audiocloud_api::{AppTaskId, FixedInstanceId, TaskReservation, TaskSpec};

use crate::db::Db;
use crate::fixed_instances::instance::InstanceActor;
use crate::fixed_instances::{NotifyInstanceReports, SetInstanceDesiredPlayState, SetInstanceParameters};
use crate::task::{NotifyTaskDeleted, NotifyTaskReservation, NotifyTaskSpec};

pub struct FixedInstancesSupervisor {
    instances: HashMap<FixedInstanceId, Instance>,
}

struct Instance {
    address: Addr<InstanceActor>,
    config:  DomainFixedInstanceConfig,
}

impl FixedInstancesSupervisor {
    pub fn new(boot: &DomainConfig, db: &Db) -> anyhow::Result<Self> {
        let mut instances = HashMap::new();
        for (id, config) in &boot.fixed_instances {
            let model = db.get_model(&id.model_id())?
                          .ok_or_else(|| anyhow!("Missing model for instance {id}"))?;
            let actor = InstanceActor::new(id.clone(), config.clone(), model)?;

            instances.insert(id.clone(),
                             Instance { address: { Supervisor::start(move |_| actor) },
                                        config:  { config.clone() }, });
        }

        Ok(Self { instances })
    }
}

impl Actor for FixedInstancesSupervisor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.restarting(ctx);
    }
}

impl Supervised for FixedInstancesSupervisor {
    fn restarting(&mut self, ctx: &mut Context<Self>) {
        self.subscribe_system_async::<NotifyTaskSpec>(ctx);
        self.subscribe_system_async::<NotifyTaskReservation>(ctx);
        self.subscribe_system_async::<NotifyTaskDeleted>(ctx);
    }
}

impl Handler<SetInstanceParameters> for FixedInstancesSupervisor {
    type Result = ();

    fn handle(&mut self, msg: SetInstanceParameters, ctx: &mut Context<FixedInstancesSupervisor>) {
        if let Some(instance) = self.instances.get(&msg.instance_id) {
            instance.do_send(msg);
        }
    }
}

impl Handler<NotifyInstanceReports> for FixedInstancesSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyInstanceReports, ctx: &mut Self::Context) -> Self::Result {
        todo!()
    }
}

impl Handler<SetInstanceDesiredPlayState> for FixedInstancesSupervisor {
    type Result = ();

    fn handle(&mut self, msg: SetInstanceDesiredPlayState, ctx: &mut Context<FixedInstancesSupervisor>) {
        if let Some(instance) = self.instances.get(&msg.instance_id) {
            instance.do_send(msg);
        }
    }
}

impl Handler<NotifyTaskReservation> for FixedInstancesSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifyTaskReservation, ctx: &mut Self::Context) -> Self::Result {
        todo!()
    }
}
