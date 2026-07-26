// The JavaScript side of the demo.
//
// This file does the whole point of the exercise: import a Rust p2p library
// that was compiled to WebAssembly, instantiate it, and call its API.

import init, { IrohNode } from "./wasm/iroh_wasm_demo.js";

const $ = (sel) => document.querySelector(sel);

main().catch((err) => {
  // Without this the page would just stop, with the reason buried in devtools.
  console.error(err);
  log(`fatal: ${err}`, "err");
});

async function main() {
  // -------------------------------------------------------------------------
  // 1. Instantiate the WASM module.
  // -------------------------------------------------------------------------
  // `init()` fetches and compiles the .wasm and wires up the JS<->Rust bindings
  // wasm-bindgen generated. Nothing Rust-side may be touched before it resolves.

  log("fetching + instantiating iroh.wasm …");
  await init();
  log("wasm module instantiated", "ok");

  // -------------------------------------------------------------------------
  // 2. Query the library's API.
  // -------------------------------------------------------------------------

  // A static method on the exported Rust type.
  log(`protocol ALPN: <code>${IrohNode.alpn()}</code>`);

  log("binding iroh endpoint …");
  const node = await IrohNode.spawn(); // async Rust constructor -> JS Promise
  log("endpoint bound", "ok");

  // Handy for poking at the API from the devtools console.
  globalThis.node = node;

  // A synchronous getter returning a Rust String.
  const endpointId = node.endpointId();
  $("#endpoint-id").textContent = endpointId;
  $("#identity").hidden = false;

  // A Rust struct serialised into a plain JS object.
  log("node.info() immediately after bind:");
  renderInfo(node.info());
  console.log("iroh info (pre-online):", node.info());

  // Relays are the only transport a browser gets, so wait for one to come up.
  log("waiting for a relay connection …");
  await node.waitOnline();
  log("relay connected — endpoint is reachable", "ok");

  log("node.info() after coming online:");
  renderInfo(node.info());
  console.table(node.info());

  // -------------------------------------------------------------------------
  // 3. Use the API: accept incoming echoes, and dial out.
  // -------------------------------------------------------------------------

  $("#share-link").href = shareLink(endpointId);
  $("#share-link").textContent = shareLink(endpointId);
  $("#cli-cmd").textContent =
    `cargo run --features cli -- connect ${endpointId} "hi from the cli"`;
  $("#live").hidden = false;

  $("#copy-id").addEventListener("click", async () => {
    await navigator.clipboard.writeText(endpointId);
    $("#copy-id").textContent = "copied";
    setTimeout(() => ($("#copy-id").textContent = "copy"), 1200);
  });

  // A Rust Stream surfaced as a JS ReadableStream.
  (async () => {
    for await (const event of iterate(node.events())) {
      console.log("incoming:", event);
      logPeer($("#incoming"), event.endpointId, describe(event));
    }
  })().catch((err) => log(`incoming event stream failed: ${err}`, "err"));

  $("#connect-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const form = new FormData(e.target);
    const peer = form.get("endpoint-id").trim();
    const payload = form.get("payload");
    if (!peer || !payload) return;

    if (peer === endpointId) {
      logPeer($("#outgoing"), peer, "that's this tab's own id — open a second tab", "err");
      return;
    }

    logPeer($("#outgoing"), peer, `dialing with payload "${payload}" …`);
    try {
      // Rust returns a ReadableStream of progress events.
      for await (const event of iterate(node.connect(peer, payload))) {
        console.log("outgoing:", event);
        logPeer($("#outgoing"), peer, describe(event), event.error ? "err" : undefined);
      }
    } catch (err) {
      // Rust errors arrive as real JS exceptions.
      logPeer($("#outgoing"), peer, `failed: ${err}`, "err");
    }
  });

  // If we were opened from a share link, prefill and auto-dial.
  const url = new URL(location.href);
  const peer = url.searchParams.get("connect");
  if (peer) {
    $("[name=endpoint-id]").value = peer;
    $("[name=payload]").value =
      url.searchParams.get("payload") || "hi from the browser";
    $("#connect-form").requestSubmit();
  }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

// ReadableStream is async-iterable in newer browsers, but not everywhere yet,
// so drive it with a reader instead of relying on Symbol.asyncIterator.
async function* iterate(stream) {
  const reader = stream.getReader();
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) return;
      yield value;
    }
  } finally {
    reader.releaseLock();
  }
}

function describe(event) {
  switch (event.type) {
    case "connected":
      return "connected";
    case "sent":
      return `sent ${event.bytesSent} byte(s)`;
    case "received":
      return `echoed back ${event.bytesReceived} byte(s): "${event.text}"`;
    case "accepted":
      return "accepted a connection";
    case "echoed":
      return `echoed ${event.bytesSent} byte(s) back`;
    case "closed":
      return event.error ? `closed with error: ${event.error}` : "closed cleanly";
    default:
      return JSON.stringify(event);
  }
}

function renderInfo(info) {
  const table = document.createElement("table");
  table.className = "info";
  for (const [key, value] of Object.entries(info)) {
    const row = table.insertRow();
    row.insertCell().textContent = key;
    const cell = row.insertCell();
    cell.textContent = Array.isArray(value)
      ? value.length
        ? value.join(", ")
        : "(none yet)"
      : String(value);
    cell.className = "value";
  }
  $("main").append(table);
}

function log(html, className) {
  const el = document.createElement("div");
  el.className = `line ${className || ""}`;
  el.innerHTML = `<span class="time">${stamp()}</span>${html}`;
  $("main").append(el);
}

function logPeer(container, peer, message, className) {
  let box = container.querySelector(`[data-peer="${peer}"]`);
  if (!box) {
    box = document.createElement("div");
    box.className = "peer";
    box.dataset.peer = peer;
    box.innerHTML = `<h4>${peer}</h4>`;
    container.append(box);
  }
  const el = document.createElement("div");
  el.className = `line ${className || ""}`;
  el.innerHTML = `<span class="time">${stamp()}</span>${message}`;
  box.append(el);
}

function stamp() {
  return new Date().toISOString().substring(11, 19);
}

function shareLink(id) {
  const url = new URL(location.href);
  url.search = "";
  url.searchParams.set("connect", id);
  url.searchParams.set("payload", "hi from the other tab");
  return url.toString();
}
