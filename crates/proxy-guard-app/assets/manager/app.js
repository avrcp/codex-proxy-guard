"use strict";

// ---------------------------------------------------------------------------
// Session token: read once from the URL hash, then strip it from the address
// bar. It lives only in this tab's JS memory.
// ---------------------------------------------------------------------------
const params = new URLSearchParams(location.hash.slice(1));
const TOKEN = params.get("token") || "";
history.replaceState(null, "", "/");

let page = "overview";
let nodeRegion = "JP";
let stateData = null;
let pollTimer = null;
let lastOperationState = "idle";
let lastStatus = "";

// ---------------------------------------------------------------------------
// API helpers
// ---------------------------------------------------------------------------
async function api(path, options = {}) {
  const headers = Object.assign({ "X-Codex-Guard-Manager": TOKEN }, options.headers || {});
  const method = options.method || (options.body ? "POST" : "GET");
  const request = {
    method,
    headers,
    body: options.body ? JSON.stringify(options.body) : undefined,
  };
  if (options.body) headers["Content-Type"] = "application/json";
  const response = await fetch(path, request);
  const text = await response.text();
  const body = text ? JSON.parse(text) : null;
  if (!response.ok) {
    const message = (body && body.message) || `HTTP ${response.status}`;
    throw new Error(message);
  }
  return body;
}

function setStatus(message, kind) {
  const bar = document.getElementById("statusbar");
  lastStatus = message;
  bar.textContent = message;
  bar.style.color = kind === "error" ? "var(--red)" : "var(--muted)";
}

function isDialogOpen() {
  return Array.from(document.querySelectorAll("dialog")).some((dialog) => dialog.open);
}

// ---------------------------------------------------------------------------
// Tabs
// ---------------------------------------------------------------------------
function switchPage(next) {
  page = next;
  document.querySelectorAll(".page").forEach((node) => node.classList.add("hidden"));
  document.getElementById(`page-${next}`).classList.remove("hidden");
  document.querySelectorAll(".tab").forEach((tab) => {
    tab.classList.toggle("active", tab.dataset.page === next);
  });
  render();
}

function bindNavigation() {
  document.querySelectorAll(".tab").forEach((tab) => {
    tab.addEventListener("click", () => switchPage(tab.dataset.page));
  });
  document.querySelectorAll(".region-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      nodeRegion = tab.dataset.region;
      document.querySelectorAll(".region-tab").forEach((other) => {
        other.classList.toggle("active", other === tab);
      });
      loadNodes();
    });
  });
  document.getElementById("close-manager").addEventListener("click", async () => {
    try {
      await api("/api/v1/manager/close", { method: "POST", body: {} });
    } catch (error) {
      setStatus(`Close failed: ${error.message}`, "error");
    }
  });
}

// ---------------------------------------------------------------------------
// Overview
// ---------------------------------------------------------------------------
function renderOverview(data) {
  document.getElementById("ov-mode").textContent = data.mode === "managed" ? "MANAGED" : "EXTERNAL";
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
      (data.manual_active ? `<div class="node-stats">Restart Guard or re-benchmark to return to AUTO.</div>` : "");
  } else if (data.mode === "managed") {
    box.innerHTML =
      `<div class="auto-flag">AUTO · JP > SG > US</div>` +
      `<div class="node-stats">No healthy node yet — run a benchmark on the Nodes page.</div>`;
  } else {
    box.innerHTML = `<div class="node-stats">External mode — no managed selection.</div>`;
  }
}

// ---------------------------------------------------------------------------
// Subscriptions
// ---------------------------------------------------------------------------
async function loadSubscriptions() {
  try {
    const subscriptions = await api("/api/v1/subscriptions");
    const list = document.getElementById("sub-list");
    list.innerHTML = "";
    if (subscriptions.length === 0) {
      list.innerHTML = `<div class="item"><span class="meta">No subscriptions yet.</span></div>`;
    }
    for (const subscription of subscriptions) {
      const item = document.createElement("div");
      item.className = "item";
      const badges = [
        subscription.active ? `<span class="badge active">ACTIVE</span>` : "",
        `<span class="badge ${subscription.last_sync_status === "failed" ? "rejected" : subscription.last_sync_status === "succeeded" ? "healthy" : "not-tested"}">${subscription.last_sync_status}</span>`,
      ].join(" ");
      const synced = subscription.last_sync_at
        ? `last sync ${new Date(subscription.last_sync_at).toLocaleString()}`
        : "never synced";
      item.innerHTML =
        `<div class="grow">` +
        `<div class="title">${escapeHtml(subscription.name)} ${badges}</div>` +
        `<div class="meta">${synced} · ${subscription.active_nodes} active · ${subscription.stale_nodes} stale</div>` +
        `</div>` +
        `<button class="ghost" data-act="activate">Activate</button>` +
        `<button class="ghost" data-act="sync">Sync</button>` +
        `<button class="ghost" data-act="edit">Edit</button>` +
        `<button class="ghost danger" data-act="delete">Delete</button>`;
      list.appendChild(item);

      const id = subscription.id;
      item.querySelector('[data-act="activate"]').addEventListener("click", () => activateSubscription(id, subscription.name));
      item.querySelector('[data-act="sync"]').addEventListener("click", () => syncSubscription(id));
      item.querySelector('[data-act="edit"]').addEventListener("click", () => openEditDialog(subscription));
      item.querySelector('[data-act="delete"]').addEventListener("click", () => deleteSubscription(id, subscription.name));
    }
  } catch (error) {
    setStatus(`Load subscriptions failed: ${error.message}`, "error");
  }
}

async function syncSubscription(id) {
  try {
    const summary = await api(`/api/v1/subscriptions/${id}/sync`, { method: "POST", body: {} });
    setStatus(
      `Synced: ${summary.imported} imported · ${summary.updated} updated · ${summary.stale} stale · ${summary.ignored_region} ignored`,
    );
    loadSubscriptions();
  } catch (error) {
    setStatus(`Sync failed: ${error.message}`, "error");
  }
}

async function activateSubscription(id, name) {
  try {
    await api(`/api/v1/subscriptions/${id}/activate`, { method: "POST", body: {} });
    setStatus(`Activated ${name}. Return to the TUI and press L to launch.`);
    loadSubscriptions();
  } catch (error) {
    setStatus(`Activate failed: ${error.message}`, "error");
  }
}

async function deleteSubscription(id, name) {
  if (!window.confirm(`Delete subscription "${name}" and its stored URL?`)) return;
  try {
    await api(`/api/v1/subscriptions/${id}`, { method: "DELETE" });
    setStatus(`Deleted ${name}.`);
    loadSubscriptions();
  } catch (error) {
    setStatus(`Delete failed: ${error.message}`, "error");
  }
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
      loadSubscriptions();
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
  document.getElementById("inspect-btn").addEventListener("click", async () => {
    const url = document.getElementById("sub-url").value.trim();
    if (!url) return;
    result.className = "inspect-result";
    result.textContent = "Inspecting…";
    try {
      const preview = await api("/api/v1/subscriptions/inspect", { method: "POST", body: { url } });
      result.className = "inspect-result ok";
      result.textContent =
        `Found ${preview.fetched} · supported ${preview.supported} · ignored region ${preview.ignored_region}` +
        ` · unsupported ${preview.unsupported}`;
    } catch (error) {
      result.className = "inspect-result err";
      result.textContent = `Inspect failed: ${error.message}`;
    }
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
      loadSubscriptions();
    } catch (error) {
      setStatus(`Add failed: ${error.message}`, "error");
    }
  }

  document.getElementById("save-sync-btn").addEventListener("click", () => saveAndSync(false));
  document.getElementById("save-sync-activate-btn").addEventListener("click", () => saveAndSync(true));
}

// ---------------------------------------------------------------------------
// Nodes + benchmark
// ---------------------------------------------------------------------------
async function loadNodes() {
  const list = document.getElementById("node-list");
  list.innerHTML = `<div class="item"><span class="meta">Loading…</span></div>`;
  try {
    const nodes = await api(`/api/v1/nodes?region=${nodeRegion}&state=all`);
    list.innerHTML = "";
    if (nodes.length === 0) {
      list.innerHTML = `<div class="item"><span class="meta">No ${nodeRegion} nodes. Sync a subscription first.</span></div>`;
      return;
    }
    const ordered = orderNodes(nodes);
    for (const node of ordered) {
      const item = document.createElement("div");
      item.className = "item";
      const statusBadge = statusClass(node.status);
      const manual = isManualSelection(node);
      const stats =
        node.status === "healthy"
          ? `score <b>${node.score}</b> · success <b>${node.success_percent}%</b> · median <b>${node.median_ms} ms</b>` +
            ` · p95 <b>${node.p95_ms} ms</b> · exit ${node.exit_stable ? "stable" : "changed"}` +
            ` · verified ${node.verified_region}`
          : node.status === "rejected"
            ? "rejected by the last benchmark"
            : "not benchmarked";
      item.innerHTML =
        `<div class="grow">` +
        `<div class="title">${escapeHtml(node.name)} ` +
        `<span class="badge ${statusBadge}">${node.status}</span>` +
        (manual ? `<span class="badge manual">SELECTED</span>` : ``) +
        `</div>` +
        `<div class="node-stats">${stats}</div>` +
        `</div>` +
        (node.status === "healthy"
          ? `<button class="ghost" data-act="select">Use for next launch</button>`
          : "");
      list.appendChild(item);
      if (node.status === "healthy") {
        item.querySelector('[data-act="select"]').addEventListener("click", () => selectNode(node));
      }
    }
  } catch (error) {
    list.innerHTML = `<div class="item"><span class="meta">Load failed: ${error.message}</span></div>`;
  }
}

function orderNodes(nodes) {
  const rank = { manual: 0, healthy: 1, "not-tested": 2, rejected: 3, stale: 4 };
  const sorted = nodes.slice().sort((left, right) => {
    const leftRank = isManualSelection(left) ? rank.manual : rank[left.status] ?? 9;
    const rightRank = isManualSelection(right) ? rank.manual : rank[right.status] ?? 9;
    if (leftRank !== rightRank) return leftRank - rightRank;
    if (left.status === "healthy" && right.status === "healthy") return right.score - left.score;
    return left.name.localeCompare(right.name);
  });
  return sorted;
}

function isManualSelection(node) {
  return !!(stateData && stateData.manual_selected && stateData.manual_selected.node_id === node.id);
}

function statusClass(status) {
  if (status === "healthy") return "healthy";
  if (status === "rejected") return "rejected";
  if (status === "stale") return "stale";
  return "not-tested";
}

async function selectNode(node) {
  try {
    await api("/api/v1/selection", { method: "POST", body: { node_id: node.id } });
    setStatus(`Manual override set: ${node.name} for next launch.`);
    await refreshState();
    loadNodes();
  } catch (error) {
    setStatus(`Selection failed: ${error.message}`, "error");
  }
}

function bindBenchmark() {
  document.getElementById("bench-auto").addEventListener("click", () => startBenchmark("auto"));
  document.getElementById("bench-all").addEventListener("click", () => startBenchmark("all"));
}

async function startBenchmark(scope) {
  try {
    await api("/api/v1/benchmark", { method: "POST", body: { scope } });
    setStatus("Benchmark started…");
  } catch (error) {
    setStatus(`Benchmark failed to start: ${error.message}`, "error");
  }
}

async function loadReports() {
  const list = document.getElementById("report-list");
  try {
    const reports = await api("/api/v1/benchmark/reports");
    list.innerHTML = "";
    if (reports.length === 0) {
      list.innerHTML = `<div class="item"><span class="meta">No benchmark reports yet.</span></div>`;
      return;
    }
    for (const report of reports) {
      const item = document.createElement("div");
      item.className = "item";
      item.innerHTML =
        `<div class="grow">` +
        `<div class="title">${escapeHtml(report.name)} <span class="badge ${statusClass(report.status)}">${report.status}</span></div>` +
        `<div class="node-stats">${report.region} · score <b>${report.score}</b> · success <b>${report.success_percent}%</b>` +
        ` · median <b>${report.median_ms} ms</b> · p95 <b>${report.p95_ms} ms</b>` +
        ` · verified ${report.verified_region} · measured ${report.measured_at ? new Date(report.measured_at).toLocaleString() : "—"}</div>` +
        `</div>`;
      list.appendChild(item);
    }
  } catch (error) {
    list.innerHTML = `<div class="item"><span class="meta">Reports failed: ${error.message}</span></div>`;
  }
}

// ---------------------------------------------------------------------------
// Polling
// ---------------------------------------------------------------------------
async function refreshState() {
  stateData = await api("/api/v1/state");
}

async function poll() {
  try {
    const [state, operation] = await Promise.all([
      api("/api/v1/state"),
      api("/api/v1/operation"),
    ]);
    stateData = state;
    renderOperation(operation);
    if (page === "overview") renderOverview(state);
    if (page === "subscriptions" && !isDialogOpen()) loadSubscriptions();
    if (page === "nodes") {
      loadNodes();
      loadReports();
    }
  } catch (error) {
    if (error.message.includes("401") || error.message.includes("invalid manager token")) {
      setStatus("Manager token rejected — close and reopen the manager from the TUI.", "error");
      clearInterval(pollTimer);
      return;
    }
    setStatus(error.message, "error");
  }
}

function renderOperation(operation) {
  if (operation.state === "benchmarking") {
    setStatus(`Benchmarking… started ${new Date(operation.started_at).toLocaleTimeString()}`);
  } else if (operation.state === "syncing") {
    setStatus("Syncing subscription…");
  } else if (operation.state === "failed") {
    setStatus(`Last operation failed: ${operation.message}`, "error");
  } else if (operation.state === "idle" && lastOperationState !== "idle") {
    if (lastOperationState === "benchmarking" && operation.last_benchmark) {
      const summary = operation.last_benchmark;
      setStatus(
        `Benchmark finished: ${summary.healthy} healthy across JP/SG/US` +
          (summary.selected ? ` · selected ${summary.selected.name}` : " · no healthy node"),
      );
    } else if (lastOperationState === "failed") {
      setStatus("Ready.");
    } else {
      setStatus("Ready.");
    }
  }
  lastOperationState = operation.state;
}

function render() {
  if (page === "overview" && stateData) renderOverview(stateData);
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------
function escapeHtml(value) {
  const div = document.createElement("div");
  div.textContent = String(value);
  return div.innerHTML;
}

function init() {
  if (!TOKEN) {
    setStatus("Missing manager token — close and reopen the manager from the TUI.", "error");
    return;
  }
  bindNavigation();
  bindAddDialog();
  bindBenchmark();
  switchPage("overview");
  pollTimer = setInterval(poll, 750);
  poll().catch((error) => setStatus(error.message, "error"));
}

document.addEventListener("DOMContentLoaded", init);
