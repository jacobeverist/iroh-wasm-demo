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
  # Deliberately musl on Linux: those builds are statically linked, so they run
  # on distros without the usual FHS dynamic loader (NixOS). The -gnu asset
  # exists for aarch64 but would fail there with "no such file or directory".
  Linux-x86_64)  WBG_TRIPLE=x86_64-unknown-linux-musl ;;
  Linux-aarch64) WBG_TRIPLE=aarch64-unknown-linux-musl ;;
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
# `ring` (pulled in by iroh's tls-ring feature) compiles C for wasm32, and not
# every clang can do that: Apple's has no wasm backend at all, and the nixpkgs
# cc-wrapper injects host flags that break the cross-compile. So rather than
# hardcode a path, probe candidates for one that actually works.
#
# These are scoped to the wasm target, so the native CLI build is unaffected.
# An explicitly-set CC_wasm32_unknown_unknown always wins — that is how
# flake.nix hands us the right compiler on NixOS.

can_target_wasm() {
  [[ -n "${1:-}" && -x "${1:-}" ]] || return 1
  echo 'int probe(void){return 0;}' |
    "$1" --target=wasm32-unknown-unknown -nostdlibinc -c -x c - -o /dev/null 2>/dev/null
}

if [[ -z "${CC_wasm32_unknown_unknown:-}" ]]; then
  for candidate in \
    /opt/homebrew/opt/llvm/bin/clang \
    /usr/local/opt/llvm/bin/clang \
    "$(command -v clang 2>/dev/null || true)"; do
    if can_target_wasm "$candidate"; then
      export CC_wasm32_unknown_unknown="$candidate"
      break
    fi
  done
fi

if [[ -z "${CC_wasm32_unknown_unknown:-}" ]]; then
  cat >&2 <<'EOF'
error: no clang on this machine can compile for wasm32-unknown-unknown, which
       `ring` requires. Install one:

         macOS   brew install llvm
         NixOS   nix develop            (flake.nix sets this up for you)
         Debian  apt install clang
EOF
  exit 1
fi

if [[ -z "${AR_wasm32_unknown_unknown:-}" ]]; then
  AR_CANDIDATE="$(dirname "$CC_wasm32_unknown_unknown")/llvm-ar"
  if [[ -x "$AR_CANDIDATE" ]]; then
    export AR_wasm32_unknown_unknown="$AR_CANDIDATE"
  elif command -v llvm-ar >/dev/null 2>&1; then
    export AR_wasm32_unknown_unknown="$(command -v llvm-ar)"
  fi
fi

echo "==> wasm cc: $CC_wasm32_unknown_unknown"

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
