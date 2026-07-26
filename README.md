# iroh in the browser — a WASM demo

A minimal, self-contained demo that takes [**iroh**](https://docs.rs/iroh/latest/iroh/) — a Rust library for peer-to-peer QUIC with NAT traversal — compiles it to `wasm32-unknown-unknown`, and drives it from a plain JavaScript module running in a browser tab.

Two tabs (or a tab and a terminal) establish a real authenticated, end-to-end-encrypted connection and echo bytes off each other.

## What it demonstrates

1. **Rust → WASM.** `src/echo.rs` is ordinary iroh code with no browser awareness. `src/wasm.rs` wraps it in `#[wasm_bindgen]`.
2. **JS imports the module.** `public/main.js` does `import init, { IrohNode } from "./wasm/iroh_wasm_demo.js"`.
3. **JS queries the API.** It calls `IrohNode.alpn()`, `await IrohNode.spawn()`, `node.endpointId()`, `node.info()`, `await node.waitOnline()`, `node.events()`, `node.connect(id, payload)`. Rust `Stream`s arrive as JS `ReadableStream`s and Rust errors as JS exceptions.
4. **The same Rust runs natively.** `src/bin/cli.rs` builds `echo.rs` into a terminal peer that can dial the browser.

The node is exposed as `globalThis.node`, so you can query the API by hand in the devtools console:

```js
node.endpointId()
node.info()
await node.connect("<peer id>", "hello")
```

## Quick start

```bash
./build.sh          # compile to wasm + generate JS bindings into public/wasm/
./serve.sh          # http://localhost:8080
```

Then open <http://localhost:8080>, wait for `relay connected`, and either:

- **browser ↔ browser** — click the share link to open a second tab, which auto-dials the first; or
- **browser ↔ native** — run the printed command:
  ```bash
  cargo run --features cli -- connect <ENDPOINT_ID> "hi from the cli"
  ```

`./build.sh --release` adds LTO, `opt-level = "z"`, symbol stripping and `wasm-opt`. Measured on this project: **15 MB debug → 2.6 MB release**, at the cost of a slower compile (~55s vs ~4s incremental).

It must be served over HTTP. Opening `index.html` as a `file://` URL fails — ES module imports and `WebAssembly.instantiateStreaming` are both blocked on that scheme.

## The one architectural caveat: browsers are relay-only

iroh's headline feature is hole-punching direct peer-to-peer connections. **You do not get that in a browser.** The sandbox has no API for sending UDP packets, so iroh falls back to tunnelling QUIC over a WebSocket to a relay server, and every byte is forwarded by the relay.

This costs latency and bandwidth, but not privacy: connections are still end-to-end encrypted between endpoints, so the relay forwards ciphertext it cannot read. `node.info().relayOnly` reports this, and `relays` lists the relay actually in use.

The native CLI peer has no such limit — it will hole-punch where it can.

## Gotchas this project already solves

These cost real time if you hit them cold. All are handled in the committed config.

### `getrandom`: the rustflag most guides tell you to set is now a no-op

Nearly every iroh-on-wasm guide, including the upstream `browser-echo` example, tells you to add:

```toml
[target.wasm32-unknown-unknown]
rustflags = ['--cfg', 'getrandom_backend="wasm_js"']
```

That was correct for **getrandom 0.3**. iroh 1.0.3 depends on **getrandom 0.4**, where `"wasm_js"` is no longer a recognised `getrandom_backend` value at all — the browser backend is selected by `#[cfg(feature = "wasm_js")]` instead (see `getrandom-0.4.3/src/backends.rs:172`). The flag silently does nothing.

The fix is a **feature**, not a flag — and because cargo unifies features across the graph, declaring it on our own dependency switches it on for iroh's copy too:

```toml
[target.'cfg(target_arch = "wasm32")'.dependencies]
getrandom = { version = "0.4", features = ["wasm_js"] }
```

Symptom if you get it wrong: `the wasm32-unknown-unknown targets are not supported by default`.

### Default features break the wasm build

iroh's default `metrics` feature does not compile for wasm:

```toml
iroh = { version = "1.0.3", default-features = false, features = ["tls-ring"] }
```

### macOS: Apple clang cannot target wasm

`tls-ring` pulls in `ring`, which compiles C. Apple's bundled clang has no wasm backend (`clang -print-targets` lists none), so the build dies with:

```
unable to create target: 'No available targets are compatible with triple wasm32-unknown-unknown'
```

`build.sh` points `cc-rs` at Homebrew LLVM for the wasm target only, leaving the native build alone:

```bash
export CC_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/clang
export AR_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/llvm-ar
```

Install it with `brew install llvm` if missing.

### wasm-bindgen CLI and crate versions must match exactly

A mismatch fails with `rust wasm file schema version ... doesn't match this binary`. `build.sh` reads the locked version out of `Cargo.lock` and fetches that exact prebuilt CLI into `.tools/`, so the two can never drift.

### Browser-only runtime traps

- `tracing_subscriber::fmt()` **must** be configured `.without_time()` — the default timer has no clock source in a browser and panics at runtime.
- `console_error_panic_hook::set_once()` turns Rust panics into readable console errors instead of `unreachable executed`.

## Layout

```
.cargo/config.toml   why the getrandom rustflag is deliberately absent
Cargo.toml           iroh (no default features) + the wasm glue
src/echo.rs          the protocol — portable, shared by both builds
src/wasm.rs          #[wasm_bindgen] surface exposed to JS
src/lib.rs           module wiring
src/bin/cli.rs       native peer (--features cli)
public/main.js       imports the module and calls the API
public/index.html    the page
build.sh / serve.sh  build and serve
```

## API exposed to JavaScript

| JS | Returns | Notes |
|---|---|---|
| `IrohNode.alpn()` | `string` | the protocol id both peers must agree on |
| `await IrohNode.spawn()` | `IrohNode` | binds an endpoint, starts accepting |
| `node.endpointId()` | `string` | this node's public identity — what a peer dials |
| `node.info()` | object | endpoint id, ALPN, relays, direct addrs, `relayOnly`, `isClosed` |
| `await node.waitOnline()` | — | resolves once a relay connection is up |
| `node.events()` | `ReadableStream` | incoming-connection events |
| `node.connect(id, payload)` | `ReadableStream` | outgoing echo progress |
| `await node.shutdown()` | — | closes the endpoint |

Event objects are tagged with `type`: `connected`, `sent`, `received`, `accepted`, `echoed`, `closed`.

## Credits

The protocol structure follows the upstream [`n0-computer/iroh-examples/browser-echo`](https://github.com/n0-computer/iroh-examples/tree/main/browser-echo), updated for iroh 1.0.3 and getrandom 0.4.

Licensed MIT OR Apache-2.0, matching iroh.
