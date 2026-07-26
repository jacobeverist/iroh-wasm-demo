// The JavaScript side of the demo.
//
// Imports a Rust p2p library (iroh + iroh-gossip) compiled to WebAssembly,
// instantiates it, and drives its API to run a gossip chat room.

import init, { IrohNode } from "./wasm/iroh_wasm_demo.js";

const $ = (sel) => document.querySelector(sel);

// Direct neighbours, i.e. peers we hold a connection to. Gossip relays through
// them, so this is a subset of who is actually in the room.
const neighbors = new Set();

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

  // Static methods on the exported Rust type.
  log(`gossip ALPN: <code>${IrohNode.alpn()}</code>`);

  log("binding iroh endpoint …");
  const node = await IrohNode.spawn(); // async Rust constructor -> JS Promise
  log("endpoint bound", "ok");
  globalThis.node = node;

  const endpointId = node.endpointId();
  $("#endpoint-id").textContent = endpointId;
  $("#identity").hidden = false;

  // A Rust struct serialised into a plain JS object.
  log("node.info() immediately after bind:");
  renderInfo(node.info());

  log("waiting for a relay connection …");
  await node.waitOnline();
  log("relay connected — endpoint is reachable", "ok");
  renderInfo(node.info());
  console.table(node.info());

  // -------------------------------------------------------------------------
  // 3. Join a gossip topic.
  // -------------------------------------------------------------------------

  const url = new URL(location.href);
  const prefillRoom = url.searchParams.get("room");
  const prefillBootstrap = url.searchParams.get("bootstrap");
  if (prefillRoom) $("[name=room]").value = prefillRoom;
  if (prefillBootstrap) $("[name=bootstrap]").value = prefillBootstrap;
  if (!$("[name=nickname]").value) {
    $("[name=nickname]").value = `tab-${endpointId.slice(0, 4)}`;
  }
  $("#join-section").hidden = false;

  $("#join-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const form = new FormData(e.target);
    const roomName = form.get("room").trim();
    const nickname = form.get("nickname").trim() || "anon";
    const bootstrap = form.get("bootstrap").trim();
    if (!roomName) return;

    // Show which swarm we're about to join before joining it.
    log(`room "${roomName}" hashes to topic <code>${IrohNode.topicForRoom(roomName)}</code>`);
    log(bootstrap ? "joining via bootstrap peer …" : "joining with no bootstrap — you are first");

    let room;
    try {
      room = await node.joinRoom(roomName, bootstrap, nickname);
    } catch (err) {
      log(`join failed: ${err}`, "err");
      return;
    }
    globalThis.room = room;

    $("#join-section").hidden = true;
    $("#live").hidden = false;
    $("#topic").textContent = room.topic();

    const share = shareLink(roomName, endpointId);
    $("#share-link").href = share;
    $("#share-link").textContent = share;
    // Dialing into a browser tab works but can take tens of seconds, so offer
    // both orderings: bootstrap off this tab, or start the CLI bare and
    // bootstrap a tab off it (which connects promptly).
    $("#cli-cmd").textContent =
      `cargo run --features cli -- join ${JSON.stringify(roomName)} --bootstrap ${endpointId}\n` +
      `# ...or start it bare and bootstrap a tab off its id (usually faster):\n` +
      `cargo run --features cli -- join ${JSON.stringify(roomName)}`;

    log(`joined as "${nickname}"`, "ok");
    renderNeighbors();

    // A Rust Stream surfaced as a JS ReadableStream.
    (async () => {
      for await (const event of iterate(room.events())) {
        console.log("gossip event", event);
        handleEvent(event);
      }
    })().catch((err) => log(`event stream failed: ${err}`, "err"));

    $("#say-form").addEventListener("submit", async (e2) => {
      e2.preventDefault();
      const text = new FormData(e2.target).get("text").trim();
      if (!text) return;
      e2.target.reset();
      try {
        await room.broadcast(text);
        // Gossip does not deliver our own messages back to us, so echo locally.
        addMessage(nickname, text, "self");
      } catch (err) {
        addMessage("!", `send failed: ${err}`, "err");
      }
    });
  });

  // Arriving via a share link means the room and bootstrap are already known.
  if (prefillRoom && prefillBootstrap) {
    $("#join-form").requestSubmit();
  }
}

function handleEvent(event) {
  switch (event.type) {
    case "neighborUp":
      neighbors.add(event.endpointId);
      renderNeighbors();
      addMessage("*", `${short(event.endpointId)} joined the swarm`, "sys");
      break;
    case "neighborDown":
      neighbors.delete(event.endpointId);
      renderNeighbors();
      addMessage("*", `${short(event.endpointId)} left the swarm`, "sys");
      break;
    case "message":
      // `from` is who *delivered* it, not necessarily who wrote it — gossip
      // relays through intermediate peers.
      addMessage(event.nickname, event.text, undefined, event.from);
      break;
    case "lagged":
      addMessage("*", "lagged — some messages were dropped", "err");
      break;
    case "error":
      addMessage("*", `error: ${event.error}`, "err");
      break;
    default:
      addMessage("*", JSON.stringify(event), "sys");
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

function addMessage(who, text, className, via) {
  const el = document.createElement("div");
  el.className = `line ${className || ""}`;
  const relay = via && via !== who ? ` <span class="time">via ${short(via)}</span>` : "";
  el.innerHTML =
    `<span class="time">${stamp()}</span><b>${escapeHtml(who)}</b> ${escapeHtml(text)}${relay}`;
  const box = $("#messages");
  box.append(el);
  box.scrollTop = box.scrollHeight;
}

function renderNeighbors() {
  const box = $("#neighbors");
  box.textContent = "";
  $("#neighbor-count").textContent = `(${neighbors.size})`;
  if (neighbors.size === 0) {
    const el = document.createElement("div");
    el.className = "line muted";
    el.textContent = "none yet — waiting for someone to connect";
    box.append(el);
    return;
  }
  for (const id of neighbors) {
    const el = document.createElement("div");
    el.className = "line";
    el.textContent = id;
    box.append(el);
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

function short(id) {
  return id.slice(0, 8);
}

function stamp() {
  return new Date().toISOString().substring(11, 19);
}

function escapeHtml(s) {
  return String(s).replace(
    /[&<>"']/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c],
  );
}

function shareLink(room, bootstrapId) {
  const url = new URL(location.href);
  url.search = "";
  url.searchParams.set("room", room);
  url.searchParams.set("bootstrap", bootstrapId);
  return url.toString();
}

$("#copy-id").addEventListener("click", async () => {
  await navigator.clipboard.writeText($("#endpoint-id").textContent);
  $("#copy-id").textContent = "copied";
  setTimeout(() => ($("#copy-id").textContent = "copy"), 1200);
});
