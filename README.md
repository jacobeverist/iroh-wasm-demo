# iroh gossip in the browser — a WASM demo

A minimal, self-contained demo that takes [**iroh**](https://docs.rs/iroh/latest/iroh/) — a Rust library for peer-to-peer QUIC with NAT traversal — plus [**iroh-gossip**](https://docs.rs/iroh-gossip/latest/iroh_gossip/), compiles them to `wasm32-unknown-unknown`, and drives them from a plain JavaScript module running in a browser tab.

Everyone who joins the same room name lands in the same gossip swarm, and chat messages propagate peer-to-peer — including to peers you never connected to directly.

## What it demonstrates

1. **Rust → WASM.** `src/chat.rs` is ordinary iroh + iroh-gossip code with no browser awareness. `src/wasm.rs` wraps it in `#[wasm_bindgen]`.
2. **JS imports the module.** `public/main.js` does `import init, { IrohNode } from "./wasm/iroh_wasm_demo.js"`.
3. **JS queries the API.** It calls `IrohNode.alpn()`, `IrohNode.topicForRoom(name)`, `await IrohNode.spawn()`, `node.endpointId()`, `node.info()`, `await node.waitOnline()`, `await node.joinRoom(room, bootstrap, nick)`, then `room.events()` / `await room.broadcast(text)`. Rust `Stream`s arrive as JS `ReadableStream`s and Rust errors as JS exceptions.
4. **The same Rust runs natively.** `src/bin/cli.rs` builds `chat.rs` into a terminal participant in the same swarm.

`node` and `room` are on `globalThis`, so you can drive the API by hand in the devtools console:

```js
node.info()
IrohNode.topicForRoom("lobby")
await room.broadcast("hello")
```

## Quick start

```bash
./build.sh          # compile to wasm + generate JS bindings into public/wasm/
./serve.sh          # http://localhost:8080
```

Open <http://localhost:8080>, wait for `relay connected`, then join a room:

- **browser ↔ browser** — join a room in the first tab, then click the share link. The second tab bootstraps off the first and auto-joins.
- **browser ↔ native** — either direction works. Fastest is to start the terminal peer first and bootstrap a tab off its endpoint id:
  ```bash
  cargo run --features cli -- join "lobby"          # prints its endpoint id
  # then open ?room=lobby&bootstrap=<that id>
  ```
  Pointing the CLI at a browser tab (`--bootstrap <tab id>`) also works but can be slow to connect — see [Bootstrapping onto a browser tab can be slow](#bootstrapping-onto-a-browser-tab-can-be-slow).

## How joining works

A gossip **topic** is 32 bytes. To spare you copying one around, the room name is hashed: `topic = blake3(room_name)`, in `chat.rs::topic_for_room`. Type the same room name and you land on the same topic. That is a plain hash, not a secret — anyone who guesses the room name can join and read along.

Messages are JSON `{nickname, text}` broadcast to the topic. Gossip **does not echo your own messages back to you**, so the UI prints what it sent locally.

The neighbours list shows *direct* connections only. Gossip relays through them, so you routinely receive messages from participants who never appear in that list — that is the difference between gossip and a point-to-point protocol.

## Gossip can't find the swarm for you

Joining needs at least one **bootstrap** peer already on the topic. The first participant joins with an empty bootstrap list and waits. Everything in the UI that passes an endpoint id around exists to solve this one problem.

### Bootstrapping *onto* a browser tab can be slow

All three directions work, but they are not equally quick to connect. Measured here:

| dialer → target | connects |
|---|---|
| browser → browser | seconds |
| browser → native | seconds |
| native → browser | **tens of seconds** — one attempt saw nothing within 25s, a retry connected within 70s |

So if a native peer is given a fresh browser tab's endpoint id and nothing seems to happen, it is very likely not broken — wait longer before concluding anything. The likely reason is that a browser endpoint's address record has to propagate before a native peer's discovery can resolve it (browsers can't use DNS directly; iroh's `dns` module is compiled out under `wasm_browser`, so the two sides publish and resolve through different paths). That explanation is inferred from the timing, not instrumented.

If you just want it to connect promptly, **start the CLI first** and bootstrap the browser off it. Once neighbours are established the swarm is fully bidirectional either way.

`./build.sh --release` adds LTO, `opt-level = "z"`, symbol stripping and `wasm-opt`. Measured on this project: **15 MB debug → 2.6 MB release**, at the cost of a slower compile (~55s vs ~4s incremental).

It must be served over HTTP. Opening `index.html` as a `file://` URL fails — ES module imports and `WebAssembly.instantiateStreaming` are both blocked on that scheme.

## Building on NixOS

> **Not verified on NixOS.** These files were written and reviewed on macOS, where no Nix is installed, so `flake.nix` / `shell.nix` have never been evaluated. Treat them as a well-researched starting point: the nixpkgs attributes referenced are confirmed to exist, but the expressions themselves are unevaluated. Fixes welcome.

```bash
nix develop          # flake.nix — pins rustc + wasm-bindgen + a wasm-capable clang
./build.sh
./serve.sh
```

Without flakes, `nix-shell` uses `shell.nix` instead. Everything after entering the shell is identical to any other platform.

### Why a shell is needed at all

Three things have to line up, and NixOS supplies none of them ambiently:

1. **A rustc with the `wasm32-unknown-unknown` std.** `flake.nix` gets this from `rust-overlay` rather than the channel's rustc.
2. **A `wasm-bindgen` binary matching `Cargo.lock` exactly** (currently 0.2.126). nixos-unstable packages these per version — `wasm-bindgen-cli_0_2_126` — which is what makes the match possible.
3. **A clang that can target wasm32**, because `ring` compiles C.

### The NixOS-specific traps

**Use the *unwrapped* clang.** The nixpkgs `cc` wrapper injects host-specific flags and a host `-target`, which break the cross-compile to wasm. Both Nix files hand `cc-rs` `llvmPackages.clang-unwrapped` instead. This is safe because `ring` compiles with `-nostdlibinc` and `-DRING_CORE_NOSTDLIBINC=1`, so it needs no libc headers — the unwrapped compiler is sufficient as well as necessary.

**Don't reach for `rustup` here.** Its downloaded toolchains are dynamically linked against a glibc loader that NixOS doesn't have at the usual path, so they fail with a confusing `no such file or directory` on a file that plainly exists. Use the Nix-provided toolchain, or `nix-ld`/an FHS env if you must.

**`build.sh`'s prebuilt-binary fallback is deliberately musl.** If no matching `wasm-bindgen` is on `PATH`, `build.sh` downloads one. On Linux it always picks the `*-unknown-linux-musl` asset, which is statically linked and therefore runs fine on NixOS. An `aarch64-unknown-linux-gnu` asset also exists upstream and would *not* run there — don't "fix" the triple table to use it.

**If your channel lacks `wasm-bindgen-cli_0_2_126`,** older channels ship a single `wasm-bindgen-cli` frozen at some version, which usually won't match `Cargo.lock`. `build.sh` will refuse to run rather than emit broken glue. Options, best first:

- point the flake at a newer nixpkgs (what `flake.nix` already does);
- drop `wasm-bindgen-cli` from the shell entirely and let `build.sh` fetch the musl binary into `.tools/`;
- pin the `wasm-bindgen` *crate* in `Cargo.toml` to whatever version your channel packages.

The `clang_multi` advice floating around iroh issue threads addresses a *different* failure (a missing 32-bit compiler surfacing as `ToolNotFound: Is 'clang' installed?`). If `build.sh` reports it can't find a wasm-capable clang, that message — not `clang_multi` — is the one to act on.

### Troubleshooting: "Relative references must start with `/`, `./`, or `../`"

The build succeeds, the page loads, and then the console rejects the module import. **This is a link error wearing a disguise, not a path problem** — there is nothing wrong with the `import` in `main.js`.

What happened: the `.wasm` came out with undefined symbols. The linker turns those into imports from a module conventionally named `env`, wasm-bindgen faithfully emits `import * as __wbg_star0 from 'env';` into the glue, and the browser refuses `env` because it is a bare specifier rather than a relative path. See [wasm-bindgen#2215](https://github.com/rustwasm/wasm-bindgen/issues/2215).

Confirm it in two commands:

```bash
grep -n "^import" public/wasm/iroh_wasm_demo.js          # a line ending `from 'env'` is the smoking gun
wasm-dis public/wasm/iroh_wasm_demo_bg.wasm | grep -oE '\(import "[^"]+" "[^"]+"' | sort -u
```

A healthy build imports **only** from `./iroh_wasm_demo_bg.js`. Anything else — `env` especially — names the module whose symbols went undefined, and the second command names the symbols themselves.

The usual cause here is the C in `ring` being compiled by a clang that injects flags the bare-metal wasm target cannot satisfy. On NixOS that is specifically the **cc wrapper**, whose hardening adds a stack protector and therefore a reference to `__stack_chk_fail` that nothing defines. Fixes, in order:

1. `nix develop` — the shell uses `llvmPackages.clang-unwrapped` and sets `hardeningDisable`, which addresses exactly this.
2. If you are rolling your own shell, don't hand `cc-rs` the wrapped `clang`; set `CC_wasm32_unknown_unknown` to `${llvmPackages.clang-unwrapped}/bin/clang` and add `hardeningDisable = [ "all" ];`.
3. If a symbol other than `__stack_chk_fail` shows up, the second command above identifies it — that points at whichever crate or C dependency isn't wasm-clean, rather than at the toolchain.

`build.sh` now fails the build if any bare specifier reaches the generated glue, and prints the offending wasm imports, so this should surface at build time rather than in the browser.

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
src/chat.rs          gossip chat node — portable, shared by both builds
src/wasm.rs          #[wasm_bindgen] surface exposed to JS
src/lib.rs           module wiring
src/bin/cli.rs       native peer (--features cli)
public/main.js       imports the module and calls the API
public/index.html    the page
build.sh / serve.sh  build and serve
flake.nix/shell.nix  NixOS dev shell (see above; unevaluated)
```

## API exposed to JavaScript

| JS | Returns | Notes |
|---|---|---|
| `IrohNode.alpn()` | `string` | the gossip ALPN, `/iroh-gossip/1`. Shared by every gossip app — unlike a custom protocol it does not identify *this* one |
| `IrohNode.topicForRoom(name)` | `string` | the topic hex a room name hashes to, without joining |
| `await IrohNode.spawn()` | `IrohNode` | binds an endpoint, mounts gossip |
| `node.endpointId()` | `string` | this node's public identity — what others bootstrap from |
| `node.info()` | object | endpoint id, ALPN, relays, direct addrs, `relayOnly`, `isClosed` |
| `await node.waitOnline()` | — | resolves once a relay connection is up |
| `await node.joinRoom(room, bootstrap, nick)` | `ChatRoom` | `bootstrap` is a comma/space separated id list; empty means "I'm first" |
| `await node.shutdown()` | — | closes the endpoint |
| `room.events()` | `ReadableStream` | topic events |
| `await room.broadcast(text)` | — | send to everyone on the topic |
| `room.topic()` / `room.nickname()` | `string` | |

Event objects are tagged with `type`: `neighborUp`, `neighborDown`, `message`, `lagged`, `error`. On a `message`, `from` is who *delivered* it, which is not necessarily who wrote it — gossip relays through intermediate peers.

## Credits

Structure follows the upstream [`n0-computer/iroh-examples`](https://github.com/n0-computer/iroh-examples) browser examples, updated for iroh 1.0.3, iroh-gossip 0.101 and getrandom 0.4.

Licensed MIT OR Apache-2.0, matching iroh.
