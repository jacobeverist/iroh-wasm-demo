//! The echo protocol, shared verbatim between the browser (wasm32) build and the
//! native CLI build.
//!
//! Nothing in this file is wasm-specific. That is the point of the demo: the
//! same iroh code drives a node in a browser tab and a node in a terminal, and
//! the two can talk to each other.

use anyhow::Result;
use async_channel::Sender;
use iroh::{
    Endpoint, EndpointId, TransportAddr,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use n0_future::{Stream, StreamExt, boxed::BoxStream, task};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tracing::info;

/// Application-Layer Protocol Negotiation id. Both sides must agree on this
/// exact byte string or the connection is refused during the TLS handshake.
pub const ALPN: &[u8] = b"iroh-wasm-demo/echo/0";

/// A live iroh endpoint with the echo protocol mounted on it.
#[derive(Debug, Clone)]
pub struct EchoNode {
    router: Router,
    accept_events: broadcast::Sender<AcceptEvent>,
}

impl EchoNode {
    /// Binds an endpoint and starts accepting echo connections.
    ///
    /// `presets::N0` uses the public relay + address-lookup servers run by n0.
    /// In a browser this is not merely a convenience: relays are the *only*
    /// transport available, because the sandbox forbids raw UDP.
    pub async fn spawn() -> Result<Self> {
        let endpoint = Endpoint::builder(presets::N0)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await?;

        let (event_sender, _rx) = broadcast::channel(128);
        let echo = Echo {
            event_sender: event_sender.clone(),
        };
        let router = Router::builder(endpoint).accept(ALPN, echo).spawn();

        Ok(Self {
            router,
            accept_events: event_sender,
        })
    }

    pub fn endpoint(&self) -> &Endpoint {
        self.router.endpoint()
    }

    /// Resolves once the endpoint has a working path to the network. In the
    /// browser that means "the WebSocket to a relay is up".
    pub async fn wait_online(&self) {
        self.endpoint().online().await;
    }

    /// A snapshot of endpoint state, shaped for JSON. This is what the JS side
    /// calls to *query* the library.
    pub fn info(&self) -> NodeInfo {
        let endpoint = self.endpoint();
        let addr = endpoint.addr();

        let mut relays = Vec::new();
        let mut direct_addrs = Vec::new();
        for transport_addr in &addr.addrs {
            match transport_addr {
                TransportAddr::Relay(url) => relays.push(url.to_string()),
                other => direct_addrs.push(format!("{other:?}")),
            }
        }

        NodeInfo {
            endpoint_id: endpoint.id().to_string(),
            alpn: String::from_utf8_lossy(ALPN).into_owned(),
            relays,
            direct_addrs,
            // Browsers cannot open UDP sockets, so hole-punching is impossible
            // and every byte is relayed (still end-to-end encrypted).
            relay_only: cfg!(target_arch = "wasm32"),
            is_closed: endpoint.is_closed(),
        }
    }

    /// Stream of events for *incoming* connections.
    pub fn accept_events(&self) -> BoxStream<AcceptEvent> {
        let receiver = self.accept_events.subscribe();
        Box::pin(BroadcastStream::new(receiver).filter_map(|event| event.ok()))
    }

    /// Dials `endpoint_id`, sends `payload`, reads the echo back. Returns a
    /// stream of progress events rather than a single future so the UI can show
    /// each stage as it happens.
    pub fn connect(
        &self,
        endpoint_id: EndpointId,
        payload: String,
    ) -> impl Stream<Item = ConnectEvent> + Unpin + use<> {
        let (event_sender, event_receiver) = async_channel::bounded(16);
        let endpoint = self.endpoint().clone();

        task::spawn(async move {
            let res = run_connect(&endpoint, endpoint_id, payload, event_sender.clone()).await;
            let error = res.as_ref().err().map(|err| err.to_string());
            event_sender.send(ConnectEvent::Closed { error }).await.ok();
        });

        Box::pin(event_receiver)
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.router.shutdown().await?;
        Ok(())
    }
}

/// Serialised straight to a JS object by `serde_wasm_bindgen`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeInfo {
    pub endpoint_id: String,
    pub alpn: String,
    pub relays: Vec<String>,
    pub direct_addrs: Vec<String>,
    pub relay_only: bool,
    pub is_closed: bool,
}

// On an enum, `rename_all` renames the VARIANTS; the fields inside each variant
// need `rename_all_fields`. Without it JS sees `bytes_sent`, not `bytesSent`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ConnectEvent {
    Connected,
    Sent { bytes_sent: u64 },
    Received { bytes_received: u64, text: String },
    Closed { error: Option<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum AcceptEvent {
    Accepted {
        endpoint_id: String,
    },
    Echoed {
        endpoint_id: String,
        bytes_sent: u64,
    },
    Closed {
        endpoint_id: String,
        error: Option<String>,
    },
}

/// The accepting half of the protocol.
#[derive(Debug, Clone)]
struct Echo {
    event_sender: broadcast::Sender<AcceptEvent>,
}

impl Echo {
    async fn handle(self, connection: Connection) -> Result<(), AcceptError> {
        let endpoint_id = connection.remote_id().to_string();
        self.event_sender
            .send(AcceptEvent::Accepted {
                endpoint_id: endpoint_id.clone(),
            })
            .ok();

        let res = self.echo_once(&connection, &endpoint_id).await;
        let error = res.as_ref().err().map(|err| err.to_string());
        self.event_sender
            .send(AcceptEvent::Closed { endpoint_id, error })
            .ok();
        res
    }

    async fn echo_once(
        &self,
        connection: &Connection,
        endpoint_id: &str,
    ) -> Result<(), AcceptError> {
        info!("accepted connection from {endpoint_id}");

        // The protocol is request/response, so we expect exactly one bi stream.
        let (mut send, mut recv) = connection.accept_bi().await?;

        // Echo every byte straight back.
        let bytes_sent = tokio::io::copy(&mut recv, &mut send).await?;

        // `finish` signals we are done writing, which terminates the peer's
        // read stream. Without it the dialer waits forever.
        send.finish()?;

        self.event_sender
            .send(AcceptEvent::Echoed {
                endpoint_id: endpoint_id.to_string(),
                bytes_sent,
            })
            .ok();

        // Let the dialer close first, so it definitely got the response.
        connection.closed().await;
        Ok(())
    }
}

impl ProtocolHandler for Echo {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        self.clone().handle(connection).await
    }
}

/// The dialing half of the protocol.
async fn run_connect(
    endpoint: &Endpoint,
    endpoint_id: EndpointId,
    payload: String,
    events: Sender<ConnectEvent>,
) -> Result<()> {
    let connection = endpoint.connect(endpoint_id, ALPN).await?;
    events.send(ConnectEvent::Connected).await?;

    let (mut send, mut recv) = connection.open_bi().await?;

    let bytes_sent = payload.len() as u64;
    send.write_all(payload.as_bytes()).await?;
    send.finish()?;
    events.send(ConnectEvent::Sent { bytes_sent }).await?;

    // Bounded read: this is a demo, and an unbounded read from a remote peer is
    // an easy way to be OOM'd.
    let echoed = recv.read_to_end(64 * 1024).await?;

    connection.close(0u8.into(), b"done");
    events
        .send(ConnectEvent::Received {
            bytes_received: echoed.len() as u64,
            text: String::from_utf8_lossy(&echoed).into_owned(),
        })
        .await?;

    Ok(())
}
