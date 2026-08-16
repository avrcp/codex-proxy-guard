"use strict";

// ---------------------------------------------------------------------------
// Session token: read once from the URL hash, then strip it from the address
// bar. It lives only in this tab's JS memory.
// ---------------------------------------------------------------------------
const params = new URLSearchParams(location.hash.slice(1));
const TOKEN = params.get("token") || "";
history.replaceState(null, "", "/");

const POLL_BUSY_MS = 800;
const POLL_IDLE_MS = 2000;
const GET_TIMEOUT_MS = 10000;
const MAX_NETWORK_FAILURES = 5;

// The single client-side model. Pages render from this snapshot only, so
// identical data never touches the DOM twice.
const model = {
  page: "overview",
  nodeRegion: "JP",
  state: null,
  operation: { state: "idle" },
  subscriptions: null,
  nodes: null,
  reports: null,
};

let pollTimer = null;
let polling = false;
let pollStopped = false;
let networkFailures = 0;
let lastOperationState = "";
let overviewSignature = "";

// ---------------------------------------------------------------------------
// API client
// ---------------------------------------------------------------------------
async function api(path, options = {}) {
  const headers = Object.assign({ "X-Codex-Guard-Manager": TOKEN }, options.headers || {});
  const method = options.method || (options.body ? "POST" : "GET");
  if (options.body) headers["Content-Type"] = "application/json";
  const request = {
    method,
    headers,
    body: options.body ? JSON.stringify(options.body) : undefined,
  };
  let response;
  if (options.body) {
    response = await fetch(path, request);
  } else {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), GET_TIMEOUT_MS);
    request.signal = controller.signal;
    try {
      response = await fetch(path, request);
    } catch (error) {
      if (error && error.name === "AbortError") {
        throw new Error("manager request timed out");
      }
      throw new Error("manager unreachable");
    } finally {
      clearTimeout(timeout);
    }
  }
  const text = await response.text();
  let body = null;
  if (text) {
    try {
      body = JSON.parse(text);
    } catch {
      body = null;
    }
  }
  if (!response.ok) {
    const error = new Error((body && body.message) || `HTTP ${response.status}`);
    error.status = response.status;
    throw error;
  }
  return body;
}

// ---------------------------------------------------------------------------
// DOM helpers
// ---------------------------------------------------------------------------
function escapeHtml(value) {
  const div = document.createElement("div");
  div.textContent = String(value);
  return div.innerHTML;
}

function setStatus(message, kind) {
  const bar = document.getElementById("statusbar");
  bar.textContent = message;
  bar.style.color = kind === "error" ? "var(--red)" : "var(--muted)";
}

function isDialogOpen() {
  return Array.from(document.querySelectorAll("dialog")).some((dialog) => dialog.open);
}

function formatTime(value) {
  return value ? new Date(value).toLocaleTimeString() : "";
}

function formatDateTime(value) {
  return value ? new Date(value).toLocaleString() : "—";
}

// Reconcile `rows` into `container` by key. Rows with an unchanged signature
// keep their element (and therefore hover/focus); changed rows are rewritten
// in place; stale rows are removed. Placeholders use a "__" key prefix.
function syncList(container, rows) {
  const existing = new Map();
  for (const child of Array.from(container.children)) {
    existing.set(child.dataset.key, child);
  }
  const seen = new Set();
  let anchor = null;
  for (const row of rows) {
    seen.add(row.key);
    let element = existing.get(row.key);
    if (!element) {
      element = document.createElement("div");
      element.className = row.key.startsWith("__") ? "item placeholder" : "item";
      element.dataset.key = row.key;
      element.dataset.sig = row.signature;
      element.innerHTML = row.html;
    } else if (element.dataset.sig !== row.signature) {
      element.dataset.sig = row.signature;
      element.innerHTML = row.html;
    }
    const expected = anchor ? anchor.nextSibling : container.firstChild;
    if (element !== expected) {
      container.insertBefore(element, expected);
    }
    anchor = element;
  }
  for (const [key, element] of existing) {
    if (!seen.has(key)) element.remove();
  }
}

function placeholderRow(message) {
  return {
    key: "__placeholder__",
    signature: message,
    html: `<span class="meta">${escapeHtml(message)}</span>`,
  };
}

// ---------------------------------------------------------------------------
// Row builders (pure; markup is escaped at the boundaries)
// ---------------------------------------------------------------------------
function isManualSelection(node) {
  return !!(
    model.state &&
    model.state.manual_selected &&
    model.state.manual_selected.node_id === node.id
  );
}

function orderNodes(nodes) {
  const rank = { manual: 0, healthy: 1, "not-tested": 2, rejected: 3, stale: 4 };
  return nodes.slice().sort((left, right) => {
    const leftRank = isManualSelection(left) ? rank.manual : rank[left.status] ?? 9;
    const rightRank = isManualSelection(right) ? rank.manual : rank[right.status] ?? 9;
    if (leftRank !== rightRank) return leftRank - rightRank;
    if (left.status === "healthy" && right.status === "healthy") return right.score - left.score;
    return left.name.localeCompare(right.name);
  });
}

function statusClass(status) {
  if (status === "healthy") return "healthy";
  if (status === "rejected") return "rejected";
  if (status === "stale") return "stale";
  return "not-tested";
}

function nodeRowHtml(node, manual) {
  const stats =
    node.status === "healthy"
      ? `score <b>${node.score}</b> · success <b>${node.success_percent}%</b> · median <b>${node.median_ms} ms</b>` +
        ` · p95 <b>${node.p95_ms} ms</b> · exit ${node.exit_stable ? "stable" : "changed"}` +
        ` · verified ${node.verified_region} · measured ${formatDateTime(node.measured_at)}`
      : node.status === "rejected"
        ? "rejected by the last benchmark"
        : "not benchmarked";
  return (
    `<div class="grow">` +
    `<div class="title">${escapeHtml(node.name)} ` +
    `<span class="badge ${statusClass(node.status)}">${node.status}</span>` +
    (manual ? `<span class="badge manual">SELECTED</span>` : ``) +
    `</div>` +
    `<div class="node-stats">${stats}</div>` +
    `</div>` +
    (node.status === "healthy"
      ? `<button class="ghost" data-act="select">Use for next launch</button>`
      : "")
  );
}

function nodeRows() {
  if (!model.nodes) return [placeholderRow("Loading…")];
  if (model.nodes.length === 0) {
    return [placeholderRow(`No ${model.nodeRegion} nodes. Sync a subscription first.`)];
  }
  return orderNodes(model.nodes).map((node) => ({
    key: node.id,
    signature: JSON.stringify([node, isManualSelection(node)]),
    html: nodeRowHtml(node, isManualSelection(node)),
  }));
}

function reportRowHtml(report) {
  return (
    `<div class="grow">` +
    `<div class="title">${escapeHtml(report.name)} ` +
    `<span class="badge ${statusClass(report.status)}">${report.status}</span></div>` +
    `<div class="node-stats">${report.region} · score <b>${report.score}</b> · success <b>${report.success_percent}%</b>` +
    ` · median <b>${report.median_ms} ms</b> · p95 <b>${report.p95_ms} ms</b>` +
    ` · verified ${report.verified_region} · measured ${formatDateTime(report.measured_at)}</div>` +
    `</div>`
  );
}

function reportRows() {
  if (!model.reports) return [placeholderRow("Loading…")];
  if (model.reports.length === 0) return [placeholderRow("No benchmark reports yet.")];
  return orderNodes(model.reports).map((report) => ({
    key: report.id,
    signature: JSON.stringify(report),
    html: reportRowHtml(report),
  }));
}

function subscriptionRowHtml(subscription) {
  const badges = [
    subscription.active ? `<span class="badge active">ACTIVE</span>` : "",
    `<span class="badge ${subscription.last_sync_status === "failed" ? "rejected" : subscription.last_sync_status === "succeeded" ? "healthy" : "not-tested"}">${subscription.last_sync_status}</span>`,
  ].join(" ");
  const synced = subscription.last_sync_at
    ? `last sync ${formatDateTime(subscription.last_sync_at)}`
    : "never synced";
  return (
    `<div class="grow">` +
    `<div class="title">${escapeHtml(subscription.name)} ${badges}</div>` +
    `<div class="meta">${synced} · ${subscription.active_nodes} active · ${subscription.stale_nodes} stale</div>` +
    `</div>` +
    `<button class="ghost" data-act="activate">Activate</button>` +
    `<button class="ghost" data-act="sync">Sync</button>` +
    `<button class="ghost" data-act="edit">Edit</button>` +
    `<button class="ghost danger" data-act="delete">Delete</button>`
  );
}

function subscriptionRows() {
  if (!model.subscriptions) return [placeholderRow("Loading…")];
  if (model.subscriptions.length === 0) return [placeholderRow("No subscriptions yet.")];
  return model.subscriptions.map((subscription) => ({
    key: subscription.id,
    signature: JSON.stringify(subscription),
    html: subscriptionRowHtml(subscription),
  }));
}

// ---------------------------------------------------------------------------
// Renderers
// ---------------------------------------------------------------------------
function renderOverview() {
  if (!model.state) return;
  const signature = JSON.stringify(model.state);
  if (signature === overviewSignature) return;
  overviewSignature = signature;
  const data = model.state;

  document.getElementById("ov-mode").textContent =
    data.mode === "managed" ? "MANAGED" : "EXTERNAL";
  document.getElementById("ov-subscription").textContent =
    data.subscription_name || (data.mode === "managed" ? "configured subscription" : "—");

  const regions = data.regions || {};
  const grid = document.getElementById("ov-regions");
  grid.innerHTML = "";
  for (const [region, active, healthy] of [
    ["JP", regions.jp_active, regions.jp_healthy],
    ["SG", regions.sg_active, regions.sg_healthy],
    ["US", regions.us_active, regions.us_healthy],
  ]) {
    const cell = document.createElement("div");
    cell.className = "region-cell";
    cell.innerHTML =
      `<div class="region">${region}</div>` +
      `<div class="counts">${active} active · ${healthy} healthy</div>`;
    grid.appendChild(cell);
  }

  const box = document.getElementById("ov-selection");
  if (data.selection) {
    const flag = data.manual_active ? "MANUAL OVERRIDE · next launch only" : "AUTO · JP > SG > US";
    const flagClass = data.manual_active ? "manual-flag" : "auto-flag";
    const selection = data.selection;
    box.innerHTML =
      `<div><span class="${flagClass}">${flag}</span></div>` +
      `<div><b>${escapeHtml(selection.name)}</b> · ${selection.region} VERIFIED</div>` +
      `<div class="node-stats">score <b>${selection.score}</b> · success <b>${selection.success_percent}%</b>` +
      ` · median <b>${selection.median_ms} ms</b> · p95 <b>${selection.p95_ms} ms</b>` +
      ` · exit ${selection.exit_stable ? "stable" : "changed"}</div>` +
      (data.manual_active
        ? `<div class="node-stats">Restart Guard or re-benchmark to return to AUTO.</div>`
        : "");
  } else if (data.mode === "managed") {
    box.innerHTML =
      `<div class="auto-flag">AUTO · JP > SG > US</div>` +
      `<div class="node-stats">No healthy node yet — run a benchmark on the Nodes page.</div>`;
  } else {
    box.innerHTML = `<div class="node-stats">External mode — no managed selection.</div>`;
  }
}

function renderSubscriptions() {
  syncList(document.getElementById("sub-list"), subscriptionRows());
}

function renderNodes() {
  syncList(document.getElementById("node-list"), nodeRows());
}

function renderReports() {
  syncList(document.getElementById("report-list"), reportRows());
}

function render() {
  if (model.page === "overview") renderOverview();
  if (model.page === "subscriptions") renderSubscriptions();
  if (model.page === "nodes") {
    renderNodes();
    renderReports();
  }
}

function progressLabel(progress) {
  if (!progress) return "preparing";
  const phase = progress.phase === "deep_scan" ? "deep scan" : "quick scan";
  return `${phase} ${progress.done}/${progress.total}`;
}

function renderOperation() {
  const operation = model.operation;
  if (operation.state === "benchmarking") {
    setStatus(
      `Benchmarking · ${progressLabel(operation.progress)} · started ${formatTime(operation.started_at)}`,
    );
  } else if (operation.state === "syncing") {
    setStatus("Syncing subscription…");
  } else if (operation.state === "failed") {
    setStatus(`Last operation failed: ${operation.message}`, "error");
  } else if (operation.state === "idle" && lastOperationState !== "idle") {
    if (lastOperationState === "benchmarking" && operation.last_benchmark) {
      const summary = operation.last_benchmark;
      setStatus(
        `Benchmark finished: ${summary.healthy} healthy` +
          (summary.selected ? ` · selected ${summary.selected.name}` : " · no healthy node"),
      );
    } else {
      setStatus("Ready.");
    }
  }
  lastOperationState = operation.state;
}

function updateBusy() {
  document.body.classList.toggle("op-busy", model.operation.state !== "idle");
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------
function actionsAllowed() {
  return model.operation.state === "idle";
}

function syncSubscription(id) {
  api(`/api/v1/subscriptions/${id}/sync`, { method: "POST", body: {} })
    .then((summary) => {
      setStatus(
        `Synced: ${summary.imported} imported · ${summary.updated} updated · ${summary.stale} stale · ${summary.ignored_region} ignored`,
      );
      wakePoll();
    })
    .catch((error) => setStatus(`Sync failed: ${error.message}`, "error"));
}

function activateSubscription(id, name) {
  api(`/api/v1/subscriptions/${id}/activate`, { method: "POST", body: {} })
    .then(() => {
      setStatus(`Activated ${name}. Return to the TUI and press L to launch.`);
      wakePoll();
    })
    .catch((error) => setStatus(`Activate failed: ${error.message}`, "error"));
}

function deleteSubscription(id, name) {
  if (!window.confirm(`Delete subscription "${name}" and its stored URL?`)) return;
  api(`/api/v1/subscriptions/${id}`, { method: "DELETE" })
    .then(() => {
      setStatus(`Deleted ${name}.`);
      wakePoll();
    })
    .catch((error) => setStatus(`Delete failed: ${error.message}`, "error"));
}

function selectNode(nodeId) {
  api("/api/v1/selection", { method: "POST", body: { node_id: nodeId } })
    .then(() => {
      setStatus("Manual override set for next launch.");
      wakePoll();
    })
    .catch((error) => setStatus(`Selection failed: ${error.message}`, "error"));
}

function startBenchmark(scope) {
  api("/api/v1/benchmark", { method: "POST", body: { scope } })
    .then(() => {
      setStatus("Benchmark started…");
      wakePoll();
    })
    .catch((error) => setStatus(`Benchmark failed to start: ${error.message}`, "error"));
}

function openEditDialog(subscription) {
  const dialog = document.getElementById("edit-sub-dialog");
  document.getElementById("edit-name").value = subscription.name;
  document.getElementById("edit-url").value = "";
  document.getElementById("edit-enabled").checked = subscription.enabled;
  dialog.showModal();
  dialog.onsubmit = async (event) => {
    event.preventDefault();
    const name = document.getElementById("edit-name").value.trim();
    const url = document.getElementById("edit-url").value.trim();
    const enabled = document.getElementById("edit-enabled").checked;
    const body = { name, enabled };
    if (url) body.url = url;
    try {
      await api(`/api/v1/subscriptions/${subscription.id}`, { method: "PATCH", body });
      setStatus("Subscription updated.");
      dialog.close();
      wakePoll();
    } catch (error) {
      setStatus(`Update failed: ${error.message}`, "error");
    }
  };
}

function bindAddDialog() {
  const dialog = document.getElementById("add-sub-dialog");
  const result = document.getElementById("inspect-result");
  document.getElementById("add-sub-btn").addEventListener("click", () => {
    result.className = "inspect-result";
    result.textContent = "";
    dialog.showModal();
  });
  document.getElementById("cancel-add-btn").addEventListener("click", () => dialog.close());
  document.getElementById("inspect-btn").addEventListener("click", () => {
    const url = document.getElementById("sub-url").value.trim();
    if (!url) return;
    result.className = "inspect-result";
    result.textContent = "Inspecting…";
    api("/api/v1/subscriptions/inspect", { method: "POST", body: { url } })
      .then((preview) => {
        result.className = "inspect-result ok";
        result.textContent =
          `Found ${preview.fetched} · supported ${preview.supported} · ignored region ${preview.ignored_region}` +
          ` · unsupported ${preview.unsupported}`;
      })
      .catch((error) => {
        result.className = "inspect-result err";
        result.textContent = `Inspect failed: ${error.message}`;
      });
  });

  async function saveAndSync(activate) {
    const name = document.getElementById("sub-name").value.trim();
    const url = document.getElementById("sub-url").value.trim();
    if (!name || !url) return;
    try {
      const created = await api("/api/v1/subscriptions", { method: "POST", body: { name, url } });
      const id = created.id;
      await api(`/api/v1/subscriptions/${id}/sync`, { method: "POST", body: {} });
      if (activate) {
        await api(`/api/v1/subscriptions/${id}/activate`, { method: "POST", body: {} });
        setStatus(`Saved, synced and activated ${name}.`);
      } else {
        setStatus(`Saved and synced ${name}.`);
      }
      dialog.close();
      wakePoll();
    } catch (error) {
      setStatus(`Add failed: ${error.message}`, "error");
    }
  }

  document.getElementById("save-sync-btn").addEventListener("click", () => saveAndSync(false));
  document
    .getElementById("save-sync-activate-btn")
    .addEventListener("click", () => saveAndSync(true));
}

// ---------------------------------------------------------------------------
// Navigation and delegated list events
// ---------------------------------------------------------------------------
function switchPage(next) {
  model.page = next;
  document.querySelectorAll(".page").forEach((node) => node.classList.add("hidden"));
  document.getElementById(`page-${next}`).classList.remove("hidden");
  document.querySelectorAll(".tab").forEach((tab) => {
    tab.classList.toggle("active", tab.dataset.page === next);
  });
  render();
  wakePoll();
}

function findSubscription(key) {
  return (model.subscriptions || []).find((subscription) => subscription.id === key);
}

function bindNavigation() {
  document.querySelectorAll(".tab").forEach((tab) => {
    tab.addEventListener("click", () => switchPage(tab.dataset.page));
  });
  document.querySelectorAll(".region-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      model.nodeRegion = tab.dataset.region;
      document.querySelectorAll(".region-tab").forEach((other) => {
        other.classList.toggle("active", other === tab);
      });
      model.nodes = null;
      renderNodes();
      wakePoll();
    });
  });
  document.getElementById("close-manager").addEventListener("click", () => {
    if (
      model.operation.state === "benchmarking" &&
      !window.confirm("A benchmark is running. Close the manager and cancel it?")
    ) {
      return;
    }
    api("/api/v1/manager/close", { method: "POST", body: {} }).catch((error) =>
      setStatus(`Close failed: ${error.message}`, "error"),
    );
  });

  document.getElementById("sub-list").addEventListener("click", (event) => {
    const button = event.target.closest("button[data-act]");
    if (!button || !actionsAllowed()) return;
    const item = button.closest(".item");
    const key = item && item.dataset.key;
    if (!key || key.startsWith("__")) return;
    const subscription = findSubscription(key);
    if (!subscription) return;
    switch (button.dataset.act) {
      case "activate":
        activateSubscription(subscription.id, subscription.name);
        break;
      case "sync":
        syncSubscription(subscription.id);
        break;
      case "edit":
        openEditDialog(subscription);
        break;
      case "delete":
        deleteSubscription(subscription.id, subscription.name);
        break;
    }
  });

  document.getElementById("node-list").addEventListener("click", (event) => {
    const button = event.target.closest("button[data-act]");
    if (!button || !actionsAllowed()) return;
    const item = button.closest(".item");
    const key = item && item.dataset.key;
    if (!key || key.startsWith("__")) return;
    if (button.dataset.act === "select") selectNode(key);
  });

  document.getElementById("bench-auto").addEventListener("click", () => startBenchmark("auto"));
  document.getElementById("bench-all").addEventListener("click", () => startBenchmark("all"));
}

// ---------------------------------------------------------------------------
// Poll scheduler: one in-flight cycle, adaptive delay, page-scoped fetches.
// ---------------------------------------------------------------------------
async function pollOnce() {
  const [state, operation] = await Promise.all([
    api("/api/v1/state"),
    api("/api/v1/operation"),
  ]);
  model.state = state;
  model.operation = operation;
  renderOperation();
  updateBusy();
  if (model.page === "overview") {
    renderOverview();
  } else if (model.page === "nodes") {
    const [nodes, reports] = await Promise.all([
      api(`/api/v1/nodes?region=${model.nodeRegion}&state=all`),
      api("/api/v1/benchmark/reports"),
    ]);
    model.nodes = nodes;
    model.reports = reports;
    renderNodes();
    renderReports();
  } else if (model.page === "subscriptions" && !isDialogOpen()) {
    model.subscriptions = await api("/api/v1/subscriptions");
    renderSubscriptions();
  }
}

function scheduleNextPoll() {
  const delay = model.operation.state === "idle" ? POLL_IDLE_MS : POLL_BUSY_MS;
  pollTimer = setTimeout(pollLoop, delay);
}

function stopPolling() {
  pollStopped = true;
  clearTimeout(pollTimer);
}

function handlePollError(error) {
  if (error && error.status === 401) {
    setStatus("Manager token rejected — close and reopen the manager from the TUI.", "error");
    stopPolling();
    return;
  }
  networkFailures += 1;
  if (networkFailures >= MAX_NETWORK_FAILURES) {
    setStatus("Manager unreachable — it may have been closed. Reopen it from the TUI.", "error");
    stopPolling();
    return;
  }
  setStatus(error.message || "manager request failed", "error");
}

async function pollLoop() {
  if (pollStopped) return;
  if (polling) {
    scheduleNextPoll();
    return;
  }
  polling = true;
  try {
    if (!document.hidden) {
      await pollOnce();
      networkFailures = 0;
    }
  } catch (error) {
    handlePollError(error);
  } finally {
    polling = false;
    if (!pollStopped) scheduleNextPoll();
  }
}

function wakePoll() {
  if (pollStopped) return;
  clearTimeout(pollTimer);
  pollTimer = setTimeout(pollLoop, 50);
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------
function init() {
  if (!TOKEN) {
    setStatus("Missing manager token — close and reopen the manager from the TUI.", "error");
    return;
  }
  bindNavigation();
  bindAddDialog();
  switchPage("overview");
  pollLoop();
}

document.addEventListener("DOMContentLoaded", init);
