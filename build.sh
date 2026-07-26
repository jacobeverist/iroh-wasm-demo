#!/usr/bin/env bash
#
# Build the Rust crate to wasm32-unknown-unknown and generate the JS bindings
# into public/wasm/.
#
#   ./build.sh            # debug build (fast to compile, large .wasm)
#   ./build.sh --release  # optimised + wasm-opt (slow to compile, small .wasm)

set -euo pipefail
cd "$(dirname "$0")"

PROFILE="debug"
CARGO_PROFILE_ARGS=()
if [[ "${1:-}" == "--release" ]]; then
  PROFILE="release"
  CARGO_PROFILE_ARGS=(--release)
fi

CRATE=iroh_wasm_demo
OUT_DIR=public/wasm

# --- 1. wasm-bindgen CLI must match the wasm-bindgen crate EXACTLY -----------
# The generated glue encodes a schema version; a mismatched CLI fails with
# "rust wasm file schema version ... doesn't match this binary". Rather than
# make you chase it, resolve the locked version and fetch that exact CLI.

if [[ ! -f Cargo.lock ]]; then
  cargo generate-lockfile
fi

WBG_VERSION=$(
  awk '/^name = "wasm-bindgen"$/ { found = 1; next }
       found && /^version = / { gsub(/[",]/, "", $3); print $3; exit }' Cargo.lock
)

if [[ -z "$WBG_VERSION" ]]; then
  echo "error: could not read the wasm-bindgen version out of Cargo.lock" >&2
  exit 1
fi

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)  WBG_TRIPLE=aarch64-apple-darwin ;;
  Darwin-x86_64) WBG_TRIPLE=x86_64-apple-darwin ;;
  Linux-x86_64)  WBG_TRIPLE=x86_64-unknown-linux-musl ;;
  Linux-aarch64) WBG_TRIPLE=aarch64-unknown-linux-gnu ;;
  *) WBG_TRIPLE="" ;;
esac

WBG_BIN=""
if command -v wasm-bindgen >/dev/null 2>&1 &&
   [[ "$(wasm-bindgen --version | awk '{print $2}')" == "$WBG_VERSION" ]]; then
  WBG_BIN=$(command -v wasm-bindgen)
else
  # Cache the matching prebuilt CLI locally instead of polluting ~/.cargo/bin.
  LOCAL_WBG=".tools/wasm-bindgen-$WBG_VERSION-$WBG_TRIPLE/wasm-bindgen"
  if [[ ! -x "$LOCAL_WBG" && -n "$WBG_TRIPLE" ]]; then
    echo "==> fetching wasm-bindgen $WBG_VERSION ($WBG_TRIPLE)"
    mkdir -p .tools
    URL="https://github.com/rustwasm/wasm-bindgen/releases/download/$WBG_VERSION/wasm-bindgen-$WBG_VERSION-$WBG_TRIPLE.tar.gz"
    curl -sSfL "$URL" | tar xz -C .tools
  fi
  if [[ -x "$LOCAL_WBG" ]]; then
    WBG_BIN="$LOCAL_WBG"
  else
    echo "error: need wasm-bindgen $WBG_VERSION; install with:" >&2
    echo "         cargo install wasm-bindgen-cli --version $WBG_VERSION" >&2
    exit 1
  fi
fi

echo "==> wasm-bindgen: $($WBG_BIN --version) ($WBG_BIN)"

# --- 2. compile ------------------------------------------------------------
# `ring` (pulled in by iroh's tls-ring feature) compiles C. Apple's clang has no
# wasm backend, so point cc-rs at Homebrew LLVM for this target only; the native
# CLI build is unaffected.

if [[ -x /opt/homebrew/opt/llvm/bin/clang ]]; then
  export CC_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/clang
  export AR_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/llvm-ar
fi

echo "==> cargo build ($PROFILE)"
# The `[@]+` guard is needed because macOS ships bash 3.2, where expanding an
# empty array under `set -u` is an "unbound variable" error.
cargo build --target wasm32-unknown-unknown \
  ${CARGO_PROFILE_ARGS[@]+"${CARGO_PROFILE_ARGS[@]}"}

# A global ~/.cargo/config.toml may redirect build.target-dir, so ask cargo
# where the artifacts actually landed instead of assuming ./target.
TARGET_DIR=$(cargo metadata --format-version 1 --no-deps |
  sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
WASM_IN="$TARGET_DIR/wasm32-unknown-unknown/$PROFILE/$CRATE.wasm"

if [[ ! -f "$WASM_IN" ]]; then
  echo "error: expected wasm at $WASM_IN but it is not there" >&2
  exit 1
fi

# --- 3. generate JS bindings ------------------------------------------------
echo "==> wasm-bindgen --target web"
rm -rf "$OUT_DIR"
BINDGEN_ARGS=(--target web --weak-refs --out-dir "$OUT_DIR")
[[ "$PROFILE" == "debug" ]] && BINDGEN_ARGS+=(--debug)
"$WBG_BIN" "$WASM_IN" "${BINDGEN_ARGS[@]}"

# --- 4. optionally shrink ---------------------------------------------------
if [[ "$PROFILE" == "release" ]] && command -v wasm-opt >/dev/null 2>&1; then
  echo "==> wasm-opt -Os"
  wasm-opt --enable-nontrapping-float-to-int --enable-bulk-memory -Os \
    -o "$OUT_DIR/${CRATE}_bg.wasm" "$OUT_DIR/${CRATE}_bg.wasm"
fi

SIZE=$(ls -lh "$OUT_DIR/${CRATE}_bg.wasm" | awk '{print $5}')
echo
echo "built $OUT_DIR/${CRATE}_bg.wasm ($SIZE)"
echo "serve it with:  ./serve.sh"
