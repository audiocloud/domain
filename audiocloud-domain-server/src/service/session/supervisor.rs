use std::collections::HashMap;

use actix::fut::LocalBoxActorFuture;
use actix::{
    fut, Actor, ActorFutureExt, Addr, Context, Handler, Message, Supervised, Supervisor, SystemService, WrapFuture,
};
use actix_broker::BrokerSubscribe;
use anyhow::anyhow;

use audiocloud_api::change::SessionState;
use audiocloud_api::newtypes::AppSessionId;
use audiocloud_api::session::Session;

use crate::data::get_boot_cfg;
use crate::service::session::messages::{
    ExecuteSessionCommand, NotifySessionSpec, NotifySessionState, SetSessionDesiredState,
};
use crate::service::session::SessionActor;

pub struct SessionsSupervisor {
    active:   HashMap<AppSessionId, Addr<SessionActor>>,
    sessions: HashMap<AppSessionId, Session>,
    state:    HashMap<AppSessionId, SessionState>,
    online:   bool,
}

impl Actor for SessionsSupervisor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.restarting(ctx);
    }
}

impl Supervised for SessionsSupervisor {
    fn restarting(&mut self, ctx: &mut Self::Context) {
        self.subscribe_system_async::<NotifySessionSpec>(ctx);
        self.subscribe_system_async::<NotifySessionState>(ctx);
    }
}

impl Default for SessionsSupervisor {
    fn default() -> Self {
        let sessions = get_boot_cfg().sessions.clone();
        Self { active: Default::default(),
               sessions,
               state: Default::default(),
               online: false }
    }
}

impl SystemService for SessionsSupervisor {}

impl Handler<NotifySessionSpec> for SessionsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifySessionSpec, ctx: &mut Self::Context) -> Self::Result {
        if let Some(session) = self.sessions.get_mut(&msg.session_id) {
            session.spec = msg.spec;
        }
    }
}

impl Handler<SetSessionDesiredState> for SessionsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: SetSessionDesiredState, ctx: &mut Self::Context) -> Self::Result {
        if let Some(session) = self.active.get_mut(&msg.session_id) {
            session.do_send(msg);
        }
    }
}

impl Handler<BecomeOnline> for SessionsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: BecomeOnline, ctx: &mut Self::Context) -> Self::Result {
        if !self.online {
            self.online = true;
            for (id, session) in &self.sessions {
                if session.time.contains_now() {
                    let actor = SessionActor::new(id, session);
                    self.active.insert(id.clone(), Supervisor::start(move |_| actor));
                }
            }
        }
    }
}

impl Handler<NotifySessionState> for SessionsSupervisor {
    type Result = ();

    fn handle(&mut self, msg: NotifySessionState, ctx: &mut Self::Context) -> Self::Result {
        self.state.insert(msg.session_id, msg.state);
    }
}

impl Handler<ExecuteSessionCommand> for SessionsSupervisor {
    type Result = LocalBoxActorFuture<Self, anyhow::Result<()>>;

    fn handle(&mut self, msg: ExecuteSessionCommand, ctx: &mut Self::Context) -> Self::Result {
        if let Some(session) = self.active.get(&msg.session_id) {
            session.send(msg)
                   .into_actor(self)
                   .map(|res, _, _| anyhow::Result::<()>::Ok(res??))
                   .boxed_local()
        } else {
            fut::err(anyhow!("Session not found")).into_actor(self).boxed_local()
        }
    }
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct BecomeOnline;
