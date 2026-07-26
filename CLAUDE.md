# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project overview

A demo that compiles [iroh](https://docs.rs/iroh/latest/iroh/) — a Rust peer-to-peer QUIC library with NAT traversal — to `wasm32-unknown-unknown` and drives it from a plain JavaScript module in a browser. Two browser tabs, or a tab and a terminal, establish a real end-to-end-encrypted connection and echo bytes off each other.

`README.md` is the user-facing document and explains each gotcha below in more depth. This file is the working summary.

## Commands

```bash
./build.sh              # cargo build (wasm) -> wasm-bindgen -> public/wasm/
./build.sh --release    # + LTO/opt-level=z/wasm-opt. 15 MB debug -> 2.6 MB release
./serve.sh [port]       # python3 http.server on public/ (default 8080)

cargo build --features cli                    # native peer
cargo run --features cli -- accept            # native node that echoes, prints its endpoint id
cargo run --features cli -- connect <ENDPOINT_ID> "payload"
```

Fast wasm feedback loop (much quicker than `./build.sh` while iterating on Rust):

```bash
CC_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/clang \
AR_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/llvm-ar \
cargo check --target wasm32-unknown-unknown
```

The env vars are **not optional** on macOS. Verified: a bare `cargo check --target wasm32-unknown-unknown` fails in ring's build script with `unable to create target: 'No available targets are compatible with triple "wasm32-unknown-unknown"'`, because `cargo check` still runs build scripts and ring compiles C. `build.sh` sets them for you; ad-hoc cargo invocations do not.

Note that piping cargo through `tail` masks its exit code — use `set -o pipefail` or read the output, or a failed build will look like it passed.

### Verifying a change

There is **no automated test suite**. Verification is running the demo:

1. `./build.sh && ./serve.sh`, open the page, confirm it reaches `relay connected` with **zero console errors**.
2. Browser ↔ browser: open the share link (`?connect=<id>&payload=…`) in a second tab; it auto-dials. Expect `connected → sent → echoed back → closed cleanly`.
3. Browser ↔ native: `cargo run --features cli -- connect <browser id> "hi"` and check the browser's *incoming* column.

Check both directions. The accept path and the dial path use different event enums, so a bug in one (a mis-serialised field, a stream that never terminates) can leave the other looking perfectly healthy.

## Architecture

**One crate, two compilation targets.** `[lib] crate-type = ["cdylib", "rlib"]`: the cdylib becomes the browser's wasm module, the rlib links into the native `cli` binary. The interesting property is that both run the *same* protocol code.

- `src/echo.rs` — the protocol and node. **Completely target-agnostic**; contains no browser or wasm awareness. This is where iroh is actually used: `Endpoint::builder(presets::N0).alpns(…).bind()`, then `Router::builder(endpoint).accept(ALPN, handler).spawn()`. Behaviour changes belong here so they apply to both targets.
- `src/wasm.rs` — the **only** wasm-specific Rust. Pure glue: `#[wasm_bindgen]` wrappers converting Rust types into browser-holdable ones and Rust errors into JS exceptions. Gated by `#[cfg(target_arch = "wasm32")]` in `src/lib.rs`, so the native build never sees it.
- `src/bin/cli.rs` — native peer (`clap`), behind the `cli` feature.
- `public/main.js` — imports the generated bindings and calls the API. No framework, no bundler.

Call chain: `main.js` → `wasm.rs` (`IrohNode`) → `echo.rs` (`EchoNode`) → iroh.

**Rust → JS boundary conventions.** Rust `Stream`s cross as JS `ReadableStream`s via `wasm_streams::ReadableStream::from_stream`. Event enums are `#[serde(tag = "type")]`, so JS switches on `event.type`. Async Rust methods become Promises; `Result<_, JsError>` becomes a thrown JS exception. Methods carry explicit `#[wasm_bindgen(js_name = …)]` to give JS camelCase.

**Build pipeline** (`build.sh`): resolve+align the wasm-bindgen CLI → `cargo build --target wasm32-unknown-unknown` → `wasm-bindgen --target web` into `public/wasm/` → `wasm-opt` on release. `public/wasm/` is generated and gitignored; never edit or commit it. `build.sh` asks `cargo metadata` where artifacts landed rather than assuming `./target`, because a global `~/.cargo/config.toml` may redirect `build.target-dir` (it does on this machine, to a shared cache).

## Gotchas

Each of these has already cost time here. Most are pinned by a comment in the file that fixes them — don't "clean up" those comments.

- **getrandom: use the feature, not the rustflag.** iroh 1.0.3 pulls **getrandom 0.4**, which selects the browser RNG via `#[cfg(feature = "wasm_js")]` (`getrandom-0.4.3/src/backends.rs:172`). The widely-copied `--cfg getrandom_backend="wasm_js"` rustflag is a getrandom **0.3** mechanism and is now an inert no-op — `"wasm_js"` isn't even a recognised `getrandom_backend` value. `.cargo/config.toml` is deliberately empty of rustflags and says so; the real switch is the `wasm_js` feature on our own `getrandom` dep, which cargo's feature unification applies to iroh's copy. Symptom if wrong: *"the wasm32-unknown-unknown targets are not supported by default"*.
- **iroh needs `default-features = false`.** Its default `metrics` feature does not build for wasm.
- **macOS: Apple clang has no wasm backend.** `tls-ring` pulls `ring`, which compiles C. `build.sh` exports `CC_wasm32_unknown_unknown` / `AR_wasm32_unknown_unknown` pointing at Homebrew LLVM (`/opt/homebrew/opt/llvm/bin/`), scoped to that target so the native build still uses Apple clang. Symptom if wrong: *"No available targets are compatible with triple wasm32-unknown-unknown"*.
- **wasm-bindgen CLI must equal the locked crate version exactly.** Mismatch fails with *"rust wasm file schema version … doesn't match this binary"*. `build.sh` reads the version from `Cargo.lock` and fetches that exact prebuilt CLI into `.tools/` (gitignored). If you bump `wasm-bindgen`, just rerun `build.sh` — don't hand-install into `~/.cargo/bin`.
- **serde: on an enum, `rename_all` renames variants, not their fields.** Variant *fields* need `rename_all_fields = "camelCase"`. Without it JS reads `event.bytesSent` as `undefined` while `event.type` still matches — so it looks like it works. Both event enums in `echo.rs` set both attributes.
- **`tracing_subscriber::fmt()` must use `.without_time()` in the browser** or it panics at runtime; there is no clock source. Keep `console_error_panic_hook::set_once()` too, or panics surface only as `unreachable executed`.
- **Browsers are relay-only.** No UDP in the sandbox means no hole-punching: every browser connection is tunnelled over a WebSocket to a relay (still end-to-end encrypted). `directAddrs` staying empty and `relayOnly: true` is correct behaviour, not a bug. The native CLI *will* hole-punch, so don't assume symmetric behaviour between the two peers.
- **Must be served over HTTP.** `file://` breaks both ES module imports and `WebAssembly.instantiateStreaming`.
- **`node.info().relays` is empty until `waitOnline()` resolves.** Reading relay state immediately after `spawn()` gives an empty list; that is timing, not failure.

## Conventions

- `echo.rs` stays portable — if something needs `wasm_bindgen`, `js_sys`, or `web_sys`, it belongs in `wasm.rs`.
- Adding a JS-visible method means: method in `wasm.rs` with `js_name`, then `./build.sh`, then use it in `main.js`. The regenerated `public/wasm/iroh_wasm_demo.d.ts` is the authoritative record of the JS surface — read it to confirm names crossed as intended.
- Check iroh APIs against the vendored source in `~/.cargo/registry/src/*/iroh-1.0.3/` rather than online docs or the upstream `browser-echo` example; that example pins iroh 1.0.0 and getrandom 0.3, and several details (`TransportAddr`'s path, the getrandom mechanism) have since changed.
