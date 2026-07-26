//! The JavaScript-facing API surface.
//!
//! Everything here is glue: converting Rust types into things a browser can
//! hold (`String`, plain objects, `ReadableStream`) and Rust errors into real
//! JS exceptions. The actual iroh work lives in [`crate::echo`].

use n0_future::{Stream, StreamExt};
use serde::Serialize;
use tracing::level_filters::LevelFilter;
use tracing_subscriber_wasm::MakeConsoleWriter;
use wasm_bindgen::{JsError, prelude::wasm_bindgen};
use wasm_streams::{ReadableStream, readable::sys::ReadableStream as JsReadableStream};

use crate::echo::{self, EchoNode};

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

/// A running iroh endpoint, owned by JavaScript.
#[wasm_bindgen]
pub struct IrohNode(EchoNode);

#[wasm_bindgen]
impl IrohNode {
    /// Binds an endpoint and mounts the echo protocol. `await IrohNode.spawn()`.
    pub async fn spawn() -> Result<IrohNode, JsError> {
        let node = EchoNode::spawn().await.map_err(to_js_err)?;
        Ok(IrohNode(node))
    }

    /// This node's public identity — the thing a peer dials.
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

    /// `ReadableStream` of events about *incoming* connections.
    pub fn events(&self) -> JsReadableStream {
        into_js_readable_stream(self.0.accept_events())
    }

    /// Dials a peer and echoes `payload` off it. Returns a `ReadableStream` of
    /// progress events; errors during the attempt arrive as a final
    /// `{type: "closed", error}` event rather than a thrown exception.
    pub fn connect(&self, endpoint_id: String, payload: String) -> Result<JsReadableStream, JsError> {
        let endpoint_id = endpoint_id
            .trim()
            .parse()
            .map_err(|_| JsError::new("not a valid endpoint id"))?;
        Ok(into_js_readable_stream(self.0.connect(endpoint_id, payload)))
    }

    /// The ALPN this demo speaks. Peers must match it exactly.
    pub fn alpn() -> String {
        String::from_utf8_lossy(echo::ALPN).into_owned()
    }

    /// Closes the endpoint and stops accepting.
    pub async fn shutdown(&self) -> Result<(), JsError> {
        self.0.shutdown().await.map_err(to_js_err)
    }
}

fn to_js_err(err: impl Into<anyhow::Error>) -> JsError {
    let err: anyhow::Error = err.into();
    JsError::new(&format!("{err:#}"))
}

/// Bridges a Rust `Stream` into a JS `ReadableStream` so the browser can drive
/// it with `for await (…)` or a plain reader loop.
fn into_js_readable_stream<T: Serialize>(stream: impl Stream<Item = T> + 'static) -> JsReadableStream {
    let stream = stream.map(|event| {
        serde_wasm_bindgen::to_value(&event)
            .map_err(|err| wasm_bindgen::JsValue::from(JsError::new(&err.to_string())))
    });
    ReadableStream::from_stream(stream).into_raw()
}
