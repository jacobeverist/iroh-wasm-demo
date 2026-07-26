#!/usr/bin/env bash
#
# Serve public/ over HTTP. The module MUST be served over http(s) -- opening
# index.html as a file:// URL fails, because ES module imports and
# WebAssembly.instantiateStreaming are both blocked on that scheme.

set -euo pipefail
cd "$(dirname "$0")"

PORT="${1:-8080}"

if [[ ! -f public/wasm/iroh_wasm_demo.js ]]; then
  echo "error: public/wasm is empty — run ./build.sh first" >&2
  exit 1
fi

echo "serving http://localhost:$PORT/"
exec python3 -m http.server "$PORT" --directory public
