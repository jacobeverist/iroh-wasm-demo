//! A gossip chat room, shared verbatim between the browser (wasm32) build and
//! the native CLI build.
//!
//! Nothing in this file is wasm-specific. The same code runs a chat participant
//! in a browser tab and in a terminal, and they talk to each other.
//!
//! ## How gossip differs from a direct protocol
//!
//! iroh-gossip is topic-based pub/sub built on top of iroh connections. You
//! subscribe to a [`TopicId`] and get a swarm: messages you broadcast are
//! relayed peer-to-peer to everyone else on that topic, including peers you
//! never connected to directly. That means a message can reach someone via a
//! third participant, and the set of *neighbours* (direct connections) is
//! usually much smaller than the set of participants.
//!
//! The one thing gossip cannot do is find the swarm for you: joining needs at
//! least one **bootstrap** peer that is already on the topic. The first
//! participant simply joins with an empty bootstrap list and waits.

use anyhow::Result;
use iroh::{
    Endpoint, EndpointId, TransportAddr,
    endpoint::presets,
    protocol::Router,
};
use iroh_gossip::{
    api::{Event, GossipSender},
    net::Gossip,
    proto::TopicId,
};
use n0_future::{Stream, StreamExt, task};
use serde::{Deserialize, Serialize};
use tracing::info;

/// Derive a topic from a human-typed room name, so two people who type the same
/// room name end up in the same swarm without exchanging a 32-byte id.
///
/// This is a plain hash, not a secret: anyone who guesses the room name can
/// join and read the messages. Gossip payloads are not encrypted to the topic —
/// the transport between peers is, but the swarm itself is public.
pub fn topic_for_room(room: &str) -> TopicId {
    let hash = blake3::hash(room.trim().as_bytes());
    TopicId::from_bytes(*hash.as_bytes())
}

/// An iroh endpoint with the gossip protocol mounted on it.
#[derive(Debug, Clone)]
pub struct ChatNode {
    router: Router,
    gossip: Gossip,
}

impl ChatNode {
    pub async fn spawn() -> Result<Self> {
        let endpoint = Endpoint::builder(presets::N0)
            .alpns(vec![iroh_gossip::ALPN.to_vec()])
            .bind()
            .await?;

        // Gossip is a protocol handler like any other: build it over the
        // endpoint, then mount it on the router under the gossip ALPN.
        let gossip = Gossip::builder().spawn(endpoint.clone());
        let router = Router::builder(endpoint)
            .accept(iroh_gossip::ALPN, gossip.clone())
            .spawn();

        Ok(Self { router, gossip })
    }

    pub fn endpoint(&self) -> &Endpoint {
        self.router.endpoint()
    }

    /// Resolves once the endpoint has a working path to the network. In the
    /// browser that means "the WebSocket to a relay is up".
    pub async fn wait_online(&self) {
        self.endpoint().online().await;
    }

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
            alpn: String::from_utf8_lossy(iroh_gossip::ALPN).into_owned(),
            relays,
            direct_addrs,
            relay_only: cfg!(target_arch = "wasm32"),
            is_closed: endpoint.is_closed(),
        }
    }

    /// Subscribe to `topic`, bootstrapping off `bootstrap` (may be empty if you
    /// are the first participant).
    ///
    /// Deliberately does NOT await `joined()`: with an empty bootstrap list
    /// there is nobody to join yet, so awaiting would hang the first peer
    /// forever. Callers learn about connectivity from `NeighborUp` events.
    pub async fn join(
        &self,
        topic: TopicId,
        bootstrap: Vec<EndpointId>,
        nickname: String,
    ) -> Result<ChatRoom> {
        info!("joining topic {topic} with {} bootstrap peer(s)", bootstrap.len());

        let subscription = self.gossip.subscribe(topic, bootstrap).await?;
        let (sender, mut receiver) = subscription.split();

        // Pump the gossip receiver into a channel we can hand out repeatedly.
        let (events_tx, events_rx) = async_channel::bounded(256);
        task::spawn(async move {
            while let Some(item) = receiver.next().await {
                let event = match item {
                    Ok(Event::NeighborUp(id)) => ChatEvent::NeighborUp {
                        endpoint_id: id.to_string(),
                    },
                    Ok(Event::NeighborDown(id)) => ChatEvent::NeighborDown {
                        endpoint_id: id.to_string(),
                    },
                    Ok(Event::Received(msg)) => decode(&msg.content, msg.delivered_from),
                    Ok(Event::Lagged) => ChatEvent::Lagged,
                    Err(err) => ChatEvent::Error {
                        error: err.to_string(),
                    },
                };
                if events_tx.send(event).await.is_err() {
                    break; // receiver dropped; nobody is listening
                }
            }
        });

        Ok(ChatRoom {
            sender,
            events: events_rx,
            nickname,
            topic,
        })
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.router.shutdown().await?;
        Ok(())
    }
}

/// A joined gossip topic: broadcast into it, and read events out of it.
#[derive(Debug, Clone)]
pub struct ChatRoom {
    sender: GossipSender,
    events: async_channel::Receiver<ChatEvent>,
    nickname: String,
    topic: TopicId,
}

impl ChatRoom {
    /// Broadcast to everyone on the topic. Note this does NOT echo back to us —
    /// gossip does not deliver your own messages, so the UI has to show what it
    /// sent locally.
    pub async fn broadcast(&self, text: String) -> Result<()> {
        let payload = serde_json::to_vec(&WireMessage {
            nickname: self.nickname.clone(),
            text,
        })?;
        self.sender.broadcast(payload.into()).await?;
        Ok(())
    }

    pub fn events(&self) -> impl Stream<Item = ChatEvent> + Unpin + use<> {
        Box::pin(self.events.clone())
    }

    pub fn topic(&self) -> TopicId {
        self.topic
    }

    pub fn nickname(&self) -> &str {
        &self.nickname
    }
}

/// What actually goes over the wire. Kept separate from `ChatEvent` so the
/// on-wire format is not accidentally coupled to the UI-facing shape.
#[derive(Debug, Serialize, Deserialize)]
struct WireMessage {
    nickname: String,
    text: String,
}

fn decode(content: &[u8], from: EndpointId) -> ChatEvent {
    let from = from.to_string();
    match serde_json::from_slice::<WireMessage>(content) {
        Ok(msg) => ChatEvent::Message {
            from,
            nickname: msg.nickname,
            text: msg.text,
        },
        // Anyone can broadcast anything to a public topic; don't let a
        // malformed payload kill the event loop.
        Err(_) => ChatEvent::Message {
            from,
            nickname: "(unknown)".to_string(),
            text: String::from_utf8_lossy(content).into_owned(),
        },
    }
}

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
// need `rename_all_fields`. Without it JS sees `endpoint_id`, not `endpointId`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ChatEvent {
    /// A direct neighbour appeared in the swarm.
    NeighborUp { endpoint_id: String },
    /// A direct neighbour went away.
    NeighborDown { endpoint_id: String },
    /// A chat message arrived. `from` is who delivered it, which is not
    /// necessarily who wrote it — that is the whole point of gossip.
    Message {
        from: String,
        nickname: String,
        text: String,
    },
    /// We fell behind and dropped messages.
    Lagged,
    Error { error: String },
}
