//! The JavaScript-facing API surface.
//!
//! Everything here is glue: converting Rust types into things a browser can
//! hold (`String`, plain objects, `ReadableStream`) and Rust errors into real
//! JS exceptions. The actual gossip work lives in [`crate::chat`].

use n0_future::{Stream, StreamExt};
use serde::Serialize;
use tracing::level_filters::LevelFilter;
use tracing_subscriber_wasm::MakeConsoleWriter;
use wasm_bindgen::{JsError, prelude::wasm_bindgen};
use wasm_streams::{ReadableStream, readable::sys::ReadableStream as JsReadableStream};

use crate::chat;

/// Runs automatically when the module is instantiated, before any JS calls in.
#[wasm_bindgen(start)]
fn start() {
    // Turn Rust panics into readable console errors instead of
    // "unreachable executed".
    console_error_panic_hook::set_once();

    tracing_subscriber::fmt()
        .with_max_level(LevelFilter::INFO)
        .with_writer(
            // Keep `trace!` off the console's noisy JS-backtrace path.
            MakeConsoleWriter::default().map_trace_level_to(tracing::Level::DEBUG),
        )
        // REQUIRED in a browser: the default timer has no clock source here and
        // panics at runtime.
        .without_time()
        .with_ansi(false)
        .init();

    tracing::info!("iroh-wasm-demo module initialised");
}

/// A running iroh endpoint with gossip mounted, owned by JavaScript.
#[wasm_bindgen]
pub struct IrohNode(chat::ChatNode);

#[wasm_bindgen]
impl IrohNode {
    /// Binds an endpoint and mounts the gossip protocol. `await IrohNode.spawn()`.
    pub async fn spawn() -> Result<IrohNode, JsError> {
        let node = chat::ChatNode::spawn().await.map_err(to_js_err)?;
        Ok(IrohNode(node))
    }

    /// This node's public identity — what another peer bootstraps from.
    #[wasm_bindgen(js_name = endpointId)]
    pub fn endpoint_id(&self) -> String {
        self.0.endpoint().id().to_string()
    }

    /// Live endpoint state as a plain JS object: endpoint id, ALPN, relay URLs,
    /// direct addresses, whether we are relay-only.
    pub fn info(&self) -> Result<wasm_bindgen::JsValue, JsError> {
        serde_wasm_bindgen::to_value(&self.0.info()).map_err(|err| JsError::new(&err.to_string()))
    }

    /// Resolves once a relay connection is established. Until this completes,
    /// `info().relays` is usually still empty.
    #[wasm_bindgen(js_name = waitOnline)]
    pub async fn wait_online(&self) {
        self.0.wait_online().await;
    }

    /// Join a gossip topic derived from `room`.
    ///
    /// `bootstrap` is a comma/space separated list of endpoint ids already on
    /// the topic; pass an empty string to be the first participant. Gossip
    /// cannot discover the swarm on its own, so without a bootstrap peer you
    /// will sit alone on the topic until somebody bootstraps off you.
    #[wasm_bindgen(js_name = joinRoom)]
    pub async fn join_room(
        &self,
        room: String,
        bootstrap: String,
        nickname: String,
    ) -> Result<ChatRoom, JsError> {
        let topic = chat::topic_for_room(&room);

        let mut peers = Vec::new();
        for token in bootstrap.split([',', ' ', '\n', '\t']) {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            let peer = token
                .parse()
                .map_err(|_| JsError::new(&format!("not a valid endpoint id: {token}")))?;
            peers.push(peer);
        }

        let room = self
            .0
            .join(topic, peers, nickname)
            .await
            .map_err(to_js_err)?;
        Ok(ChatRoom(room))
    }

    /// The gossip ALPN. All participants necessarily share it — unlike a custom
    /// protocol, the ALPN does not identify *this* application.
    pub fn alpn() -> String {
        String::from_utf8_lossy(iroh_gossip::ALPN).into_owned()
    }

    /// The topic id a given room name hashes to, as hex. Exposed so the UI can
    /// show which swarm it is about to join.
    #[wasm_bindgen(js_name = topicForRoom)]
    pub fn topic_for_room(room: String) -> String {
        chat::topic_for_room(&room).to_string()
    }

    /// Closes the endpoint and stops accepting.
    pub async fn shutdown(&self) -> Result<(), JsError> {
        self.0.shutdown().await.map_err(to_js_err)
    }
}

/// A joined gossip topic.
#[wasm_bindgen]
pub struct ChatRoom(chat::ChatRoom);

#[wasm_bindgen]
impl ChatRoom {
    /// `ReadableStream` of topic events: neighbours coming and going, and
    /// messages arriving.
    pub fn events(&self) -> JsReadableStream {
        into_js_readable_stream(self.0.events())
    }

    /// Broadcast to everyone on the topic.
    ///
    /// Gossip does not deliver your own messages back to you, so the UI is
    /// responsible for displaying what it sent.
    pub async fn broadcast(&self, text: String) -> Result<(), JsError> {
        self.0.broadcast(text).await.map_err(to_js_err)
    }

    /// The 32-byte topic id, as hex.
    pub fn topic(&self) -> String {
        self.0.topic().to_string()
    }

    pub fn nickname(&self) -> String {
        self.0.nickname().to_string()
    }
}

fn to_js_err(err: impl Into<anyhow::Error>) -> JsError {
    let err: anyhow::Error = err.into();
    JsError::new(&format!("{err:#}"))
}

/// Bridges a Rust `Stream` into a JS `ReadableStream` so the browser can drive
/// it with `for await (…)` or a plain reader loop.
fn into_js_readable_stream<T: Serialize>(
    stream: impl Stream<Item = T> + 'static,
) -> JsReadableStream {
    let stream = stream.map(|event| {
        serde_wasm_bindgen::to_value(&event)
            .map_err(|err| wasm_bindgen::JsValue::from(JsError::new(&err.to_string())))
    });
    ReadableStream::from_stream(stream).into_raw()
}
