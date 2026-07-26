//! iroh + iroh-gossip compiled to WebAssembly, exposed to JavaScript.
//!
//! `chat` is portable Rust shared with the native CLI binary; `wasm` is the
//! thin `#[wasm_bindgen]` layer that only exists for the browser build.

pub mod chat;

#[cfg(target_arch = "wasm32")]
mod wasm;

#[cfg(target_arch = "wasm32")]
pub use wasm::*;
