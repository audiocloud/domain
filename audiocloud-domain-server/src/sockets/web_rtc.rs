use std::sync::Arc;

use actix::fut::LocalBoxActorFuture;
use actix::{
    fut, Actor, ActorContext, ActorFutureExt, ActorTryFutureExt, AsyncContext, Context, ContextFutureSpawner, Handler,
    Message, WrapFuture,
};
use anyhow::anyhow;
use clap::Args;
use futures::executor::block_on;
use futures::FutureExt;
use once_cell::sync::OnceCell;
use tracing::*;
use webrtc::api::setting_engine::SettingEngine;
use webrtc::api::{APIBuilder, API};
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice::udp_network::{EphemeralUDP, UDPNetwork};
use webrtc::ice_transport::ice_candidate::{RTCIceCandidate, RTCIceCandidateInit};
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::RTCPeerConnection;

use audiocloud_api::SocketId;

use crate::sockets::get_sockets_supervisor;
use crate::sockets::messages::{SocketReceived, SocketSend};

static WEB_RTC_API: OnceCell<API> = OnceCell::new();

#[derive(Args, Clone, Debug)]
pub struct WebRtcOpts {
    /// Enable WebRTC transport support (used only if the app supports it as well)
    #[clap(long, env)]
    enable_web_rtc: bool,

    /// List of ICE servers to use for WebRTC connections
    #[clap(long, env, default_value = "stun:stun.google.com:19302")]
    ice_servers: Vec<String>,

    /// Beginning of UDP port range to use for WebRTC (inclusive)
    #[clap(long, env, default_value = "30000")]
    web_rtc_port_min: u16,

    /// End of UDP port range to use for WebRTC (inclusivec)
    #[clap(long, env, default_value = "40000")]
    web_rtc_port_max: u16,

    /// Maximum number of retransmits before dropping data
    #[clap(long, env, default_value = "16")]
    web_rtc_max_retransmits: u16,

    /// Use native WebRTC ordering of packets instead of reordering in the clients
    #[clap(long, env)]
    web_rtc_use_native_ordering: bool,
}

impl WebRtcOpts {
    pub fn get_ice_servers(&self) -> Vec<RTCIceServer> {
        self.ice_servers
            .iter()
            .map(|ice_server| RTCIceServer { urls: vec![ice_server.clone()],
                                             ..Default::default() })
            .collect()
    }
}

pub struct WebRtcActor {
    peer_connection: RTCPeerConnection,
    data_channel:    Arc<RTCDataChannel>,
    id:              SocketId,
}

impl WebRtcActor {
    pub async fn new(id: SocketId, remote_description: String, opts: &WebRtcOpts) -> anyhow::Result<(Self, String)> {
        let api = WEB_RTC_API.get()
                             .ok_or_else(|| anyhow!("WebRTC engine not initialized"))?;

        let rtc_config = RTCConfiguration { ice_servers: opts.get_ice_servers(),
                                            ..Default::default() };

        let dc_config = RTCDataChannelInit { ordered: Some(opts.web_rtc_use_native_ordering),
                                             max_retransmits: Some(opts.web_rtc_max_retransmits),
                                             protocol: Some("audiocloud-events".to_owned()),
                                             ..Default::default() };

        let peer_connection = api.new_peer_connection(rtc_config).await?;
        let data_channel = peer_connection.create_data_channel("data", Some(dc_config)).await?;

        let local_description = peer_connection.create_offer(None).await?;
        peer_connection.set_local_description(local_description.clone()).await?;

        peer_connection.set_remote_description(serde_json::from_str(&remote_description)?)
                       .await?;

        let local_description = serde_json::to_string(&local_description)?;

        Ok((Self { peer_connection,
                   data_channel,
                   id },
            local_description))
    }
}

impl Actor for WebRtcActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        debug!(ice_state = ?self.peer_connection.ice_connection_state(),
               data_channel_state = ?self.data_channel.ready_state(),
               "WebRTC connection started");

        let addr = ctx.address();
        self.data_channel.on_close({
                             let addr = addr.clone();
                             Box::new(move || {
                                 let addr = addr.clone();
                                 Box::pin(async move {
                                     let _ = addr.send(Closed).await;
                                 })
                             })
                         });

        self.data_channel.on_message({
                             let addr = addr.clone();
                             Box::new(move |data| {
                                 let addr = addr.clone();
                                 Box::pin(async move {
                                     let _ = addr.send(OnMessage(data)).await;
                                 })
                             })
                         });
    }
}

impl Handler<Closed> for WebRtcActor {
    type Result = ();

    fn handle(&mut self, _: Closed, ctx: &mut Self::Context) {
        debug!("DataChannel closed, Closing WebRTC connection");
        let _ = block_on(self.peer_connection.close());
        ctx.stop();
    }
}

impl Handler<OnMessage> for WebRtcActor {
    type Result = ();

    fn handle(&mut self, msg: OnMessage, ctx: &mut Self::Context) -> Self::Result {
        get_sockets_supervisor().send(SocketReceived::Bytes(self.id.clone(), msg.0.data))
                                .map(drop)
                                .into_actor(self)
                                .spawn(ctx);
    }
}

impl Handler<SocketSend> for WebRtcActor {
    type Result = ();

    fn handle(&mut self, msg: SocketSend, ctx: &mut Self::Context) -> Self::Result {
        match msg {
            SocketSend::Bytes(bytes) => {
                let data_channel = self.data_channel.clone();
                let fut = async move { data_channel.send(&bytes).await };
                fut.into_actor(self)
                   .map(|res, actor, ctx| {
                       if let Err(error) = res {
                           error!(?error, "Failed to send data to WebRTC peer");
                           ctx.stop();
                       }
                   })
                   .spawn(ctx);
            }
            SocketSend::Text(text) => {
                error!(%text, "WebRTC actor  does not (yet) support sending text messages");
            }
        }
    }
}

impl Handler<AddIceCandidate> for WebRtcActor {
    type Result = anyhow::Result<()>;

    fn handle(&mut self, msg: AddIceCandidate, ctx: &mut Self::Context) -> Self::Result {
        match serde_json::from_str::<RTCIceCandidateInit>(&msg.candidate) {
            Ok(candidate) => {
                block_on(self.peer_connection.add_ice_candidate(candidate))?;
                Ok(())
            }
            Err(error) => {
                warn!(%error, id = %self.id, "Failed to parse ICE candidate");
                Err(error.into())
            }
        }
    }
}

#[derive(Message)]
#[rtype(result = "()")]
struct OnMessage(DataChannelMessage);

#[derive(Message)]
#[rtype(result = "()")]
struct Closed;

#[derive(Message)]
#[rtype(result = "anyhow::Result<()>")]
pub struct AddIceCandidate {
    pub candidate: String,
}

pub fn init(opts: &WebRtcOpts) -> anyhow::Result<()> {
    let mut settings = SettingEngine::default();
    settings.set_udp_network(UDPNetwork::Ephemeral(EphemeralUDP::new(opts.web_rtc_port_min, opts.web_rtc_port_max)?));
    let api = APIBuilder::default().with_setting_engine(settings).build();

    WEB_RTC_API.set(api)
               .map_err(|_| anyhow!("WebRTC API already initialized"))?;

    Ok(())
}
