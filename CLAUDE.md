# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project overview

A demo that compiles [iroh](https://docs.rs/iroh/latest/iroh/) — a Rust peer-to-peer QUIC library with NAT traversal — plus [iroh-gossip](https://docs.rs/iroh-gossip/latest/iroh_gossip/) to `wasm32-unknown-unknown`, and drives them from a plain JavaScript module in a browser. Participants who join the same room name share a gossip swarm and chat; messages propagate peer-to-peer, including via intermediate peers.

`README.md` is the user-facing document and explains each gotcha below in more depth. This file is the working summary.

## Commands

```bash
./build.sh              # cargo build (wasm) -> wasm-bindgen -> public/wasm/
./build.sh --release    # + LTO/opt-level=z/wasm-opt. 15 MB debug -> 2.6 MB release
./serve.sh [port]       # python3 http.server on public/ (default 8080)

cargo build --features cli                    # native peer
cargo run --features cli -- join "lobby"      # join a room, print endpoint id, relay stdin into it
cargo run --features cli -- join "lobby" --bootstrap <ENDPOINT_ID> --nick alice
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
2. Browser ↔ browser: join a room in tab A, open the share link (`?room=…&bootstrap=…`) in tab B; it auto-joins. Expect a `neighborUp` on both, then a message sent from B to appear in A annotated `via <peer>`.
3. Browser ↔ native: start `cargo run --features cli -- join "lobby"`, then open `?room=lobby&bootstrap=<cli id>`. Messages should flow both ways.

Send in **both** directions. Gossip deliberately does not echo your own messages back, so the local echo in the UI and the received path are entirely separate code — one can work while the other is broken.

**Be patient when a native peer dials a browser tab.** That direction has been observed to take tens of seconds (nothing within 25s on one attempt; connected within 70s on a retry). A short timeout will make a working setup look broken — do not conclude it's broken and "fix" something. Bootstrapping the browser off the CLI connects in seconds.

## Architecture

**One crate, two compilation targets.** `[lib] crate-type = ["cdylib", "rlib"]`: the cdylib becomes the browser's wasm module, the rlib links into the native `cli` binary. The interesting property is that both run the *same* protocol code.

- `src/chat.rs` — the gossip node and room. **Completely target-agnostic**; contains no browser or wasm awareness. This is where iroh is actually used: `Endpoint::builder(presets::N0).alpns(…).bind()`, then `Gossip::builder().spawn(endpoint)` mounted via `Router::builder(endpoint).accept(iroh_gossip::ALPN, gossip)`. Behaviour changes belong here so they apply to both targets.
- `src/wasm.rs` — the **only** wasm-specific Rust. Pure glue: `#[wasm_bindgen]` wrappers converting Rust types into browser-holdable ones and Rust errors into JS exceptions. Gated by `#[cfg(target_arch = "wasm32")]` in `src/lib.rs`, so the native build never sees it.
- `src/bin/cli.rs` — native peer (`clap`), behind the `cli` feature.
- `public/main.js` — imports the generated bindings and calls the API. No framework, no bundler.

Call chain: `main.js` → `wasm.rs` (`IrohNode`, `ChatRoom`) → `chat.rs` (`ChatNode`, `ChatRoom`) → iroh-gossip → iroh.

**Gossip's shape, which drives the API.** You subscribe to a `TopicId` and get a swarm, not a connection. `GossipTopic::split()` gives a sender plus a receiver stream; `chat.rs` pumps that receiver into an `async_channel` so `events()` can be handed out repeatedly. A topic is derived from the room name via `blake3` (`topic_for_room`) so users type a name instead of 32 bytes — a plain hash, not a secret.

**Rust → JS boundary conventions.** Rust `Stream`s cross as JS `ReadableStream`s via `wasm_streams::ReadableStream::from_stream`. Event enums are `#[serde(tag = "type")]`, so JS switches on `event.type`. Async Rust methods become Promises; `Result<_, JsError>` becomes a thrown JS exception. Methods carry explicit `#[wasm_bindgen(js_name = …)]` to give JS camelCase.

**Build pipeline** (`build.sh`): resolve+align the wasm-bindgen CLI → `cargo build --target wasm32-unknown-unknown` → `wasm-bindgen --target web` into `public/wasm/` → `wasm-opt` on release. `public/wasm/` is generated and gitignored; never edit or commit it. `build.sh` asks `cargo metadata` where artifacts landed rather than assuming `./target`, because a global `~/.cargo/config.toml` may redirect `build.target-dir` (it does on this machine, to a shared cache).

## Gotchas

Each of these has already cost time here. Most are pinned by a comment in the file that fixes them — don't "clean up" those comments.

- **getrandom: use the feature, not the rustflag.** iroh 1.0.3 pulls **getrandom 0.4**, which selects the browser RNG via `#[cfg(feature = "wasm_js")]` (`getrandom-0.4.3/src/backends.rs:172`). The widely-copied `--cfg getrandom_backend="wasm_js"` rustflag is a getrandom **0.3** mechanism and is now an inert no-op — `"wasm_js"` isn't even a recognised `getrandom_backend` value. `.cargo/config.toml` is deliberately empty of rustflags and says so; the real switch is the `wasm_js` feature on our own `getrandom` dep, which cargo's feature unification applies to iroh's copy. Symptom if wrong: *"the wasm32-unknown-unknown targets are not supported by default"*.
- **iroh needs `default-features = false`.** Its default `metrics` feature does not build for wasm.
- **iroh-gossip needs `default-features = false, features = ["net"]`.** Its defaults are `["net", "metrics"]`: `metrics` must go for the same reason as iroh's, but `net` is **not** optional — it gates the whole network layer including `Gossip` and `iroh_gossip::ALPN`. Dropping defaults without re-adding `net` fails with `not found in iroh_gossip` plus a cascade of type-inference errors that look unrelated to features.
- **Gossip does not deliver your own messages back to you.** The UI must echo locally what it broadcast. If you "fix" a missing message by removing the local echo, sent messages vanish from your own view.
- **Joining needs a bootstrap peer.** `subscribe(topic, bootstrap)` with an empty list is legal and means "I am the first" — it does not fail, it just sits there. Do not `await joined()` on that path or the first peer hangs forever; `chat.rs` deliberately reports connectivity through `NeighborUp` events instead.
- **Chat text arrives from arbitrary peers on a public topic** — `main.js` HTML-escapes it. Keep that; a topic is guessable from its room name, so anyone can broadcast into it.
- **`ring` needs a wasm-capable clang, and the obvious one usually isn't.** Apple clang has no wasm backend; the nixpkgs `cc` wrapper injects host flags that break the cross-compile. `build.sh` probes candidates by actually compiling a snippet to wasm32 and exports `CC_wasm32_unknown_unknown` / `AR_wasm32_unknown_unknown` for that target only, so the native build is untouched. A pre-set `CC_wasm32_unknown_unknown` always wins — that is how the Nix shell injects the right compiler. Symptom if wrong: *"No available targets are compatible with triple wasm32-unknown-unknown"*.
- **NixOS is supported via `flake.nix` / `shell.nix` but has never been evaluated** (this project was developed on macOS with no Nix installed). If touching them, don't switch the Linux wasm-bindgen download triple from musl to gnu — the musl asset is static and is the reason the fallback works on NixOS — and keep the `wasm-bindgen-cli_0_2_*` attribute in step with `Cargo.lock`. See the NixOS section of `README.md`.
- **wasm-bindgen CLI must equal the locked crate version exactly.** Mismatch fails with *"rust wasm file schema version … doesn't match this binary"*. `build.sh` reads the version from `Cargo.lock` and fetches that exact prebuilt CLI into `.tools/` (gitignored). If you bump `wasm-bindgen`, just rerun `build.sh` — don't hand-install into `~/.cargo/bin`.
- **serde: on an enum, `rename_all` renames variants, not their fields.** Variant *fields* need `rename_all_fields = "camelCase"`. Without it JS reads `event.endpointId` as `undefined` while `event.type` still matches — so it looks like it works. `ChatEvent` in `chat.rs` sets both attributes.
- **`tracing_subscriber::fmt()` must use `.without_time()` in the browser** or it panics at runtime; there is no clock source. Keep `console_error_panic_hook::set_once()` too, or panics surface only as `unreachable executed`.
- **Browsers are relay-only.** No UDP in the sandbox means no hole-punching: every browser connection is tunnelled over a WebSocket to a relay (still end-to-end encrypted). `directAddrs` staying empty and `relayOnly: true` is correct behaviour, not a bug. The native CLI *will* hole-punch, so don't assume symmetric behaviour between the two peers.
- **"Relative references must start with `./`" in the browser is a LINK error, not a path error.** Undefined symbols in the `.wasm` become imports from module `env`; wasm-bindgen emits `import * as __wbg_star0 from 'env';`; the browser rejects the bare specifier. Don't go hunting through `main.js` — its import is fine. `build.sh` now fails the build on any bare specifier and prints the offending wasm imports. Diagnose with `wasm-dis …_bg.wasm | grep -oE '\(import "[^"]+" "[^"]+"'`; a healthy build imports only from `./iroh_wasm_demo_bg.js`. Usual cause is a clang that injects flags wasm can't satisfy (the nixpkgs cc **wrapper** adds a stack protector → `__stack_chk_fail`). See the troubleshooting section in `README.md`.
- **Must be served over HTTP.** `file://` breaks both ES module imports and `WebAssembly.instantiateStreaming`.
- **`node.info().relays` is empty until `waitOnline()` resolves.** Reading relay state immediately after `spawn()` gives an empty list; that is timing, not failure.

## Conventions

- `chat.rs` stays portable — if something needs `wasm_bindgen`, `js_sys`, or `web_sys`, it belongs in `wasm.rs`.
- Adding a JS-visible method means: method in `wasm.rs` with `js_name`, then `./build.sh`, then use it in `main.js`. The regenerated `public/wasm/iroh_wasm_demo.d.ts` is the authoritative record of the JS surface — read it to confirm names crossed as intended.
- Check iroh APIs against the vendored source in `~/.cargo/registry/src/*/iroh-1.0.3/` rather than online docs or the upstream `browser-echo` example; that example pins iroh 1.0.0 and getrandom 0.3, and several details (`TransportAddr`'s path, the getrandom mechanism) have since changed.
