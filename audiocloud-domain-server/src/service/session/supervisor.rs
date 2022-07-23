use actix::{Actor, Addr, Context, Handler, Supervised, Supervisor, SystemService};
use std::collections::HashMap;
use actix_broker::BrokerSubscribe;
use audiocloud_api::change::SessionState;
use audiocloud_api::newtypes::AppSessionId;
use audiocloud_api::session::Session;
use crate::data::get_boot_cfg;
use crate::service::session::{BecomeOnline, NotifySessionSpec, NotifySessionState, SessionActor, SetSessionDesiredState};

pub struct SessionsSupervisor {
    active:   HashMap<AppSessionId, Addr<SessionActor>>,
    sessions: HashMap<AppSessionId, Session>,
    state:    HashMap<AppSessionId, SessionState>,
    online:   bool,
}

impl Actor for SessionsSupervisor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {}
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
