"use strict";
// ccsnap UI. Every action is one ccsnap command sent to POST /api/run; the page only displays.

// The key arrives in the link's fragment (#t=…), which the browser never sends to the server.
// Keep it for reloads of this tab and take it out of the address bar.
function takeToken() {
  const m = location.hash.match(/^#t=([0-9a-f]+)$/);
  if (m) {
    try {
      sessionStorage.setItem("ccsnap-token", m[1]);
    } catch {
      // Storage can be unavailable; the key still works until the tab reloads.
    }
    history.replaceState(null, "", location.pathname);
    return m[1];
  }
  try {
    return sessionStorage.getItem("ccsnap-token") || "";
  } catch {
    return "";
  }
}
let TOKEN = takeToken();
const main = document.getElementById("main");
const state = { view: "list", snapshot: null, tab: "edit", version: null, hours: 24 };
const pending = new Map(); // snapshot name → unsaved edit ops, as `ccsnap edit --batch` takes them
let dragged = null; // the tab being dragged: { ws, pos }

// `reconnect`: the page has no usable key; render() then offers the Reconnect button.
class CommandError extends Error {
  constructor(message, exit, reconnect = false) {
    super(message);
    this.exit = exit;
    this.reconnect = reconnect;
  }
}

const START_HINT = "Start it with the palette command \"ccsnap: Open UI\" or `ccsnap ui`.";

// Runs `ccsnap <args> --json`. Resolves to { exit, code, result }; rejects on errors.
async function ccsnap(args, input) {
  if (!TOKEN) throw new CommandError("This page has no key yet.", 1, true);
  let r;
  try {
    r = await fetch("/api/run", {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-Ccsnap-Token": TOKEN },
      body: JSON.stringify(input === undefined ? { args } : { args, input }),
    });
  } catch {
    throw new CommandError(`The ccsnap UI server is not running. ${START_HINT}`, 1);
  }
  if (r.status === 401) {
    throw new CommandError("The ccsnap UI was restarted, so this page's key is old.", 1, true);
  }
  if (!r.ok) throw new CommandError(await r.text(), 1);
  const body = await r.json();
  if (body.result && body.result.error) throw new CommandError(body.result.error.message, body.exit);
  return body;
}

// h("div", { class: "x", onclick: f }, child, "text", [more])
function h(tag, attrs, ...children) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v === false || v === null || v === undefined) continue;
    if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
    else if (k === "class") el.className = v;
    else el.setAttribute(k, v === true ? "" : v);
  }
  for (const c of children.flat(Infinity)) {
    if (c === null || c === undefined || c === false) continue;
    el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return el;
}

function toast(message, kind = "info") {
  const t = document.getElementById("toast");
  t.textContent = message;
  t.className = `toast ${kind}`;
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => { t.className = "toast hidden"; }, 5000);
}

// Runs an action and shows its error instead of throwing.
async function guard(fn) {
  try {
    await fn();
  } catch (e) {
    toast(e.message, "error");
  }
}

function when(iso) { return iso ? new Date(iso).toLocaleString() : ""; }
function plural(n, word) { return `${n} ${word}${n === 1 ? "" : "s"}`; }
function opsOf(name) {
  if (!pending.has(name)) pending.set(name, []);
  return pending.get(name);
}
function tabLabel(t) { return t.type === "claude" ? "Claude" : t.command ? "Command" : "Shell"; }

function go(view, extra = {}) {
  Object.assign(state, { view }, extra);
  render();
}

async function render() {
  for (const b of document.querySelectorAll("nav button")) {
    b.classList.toggle("active", b.dataset.view === (state.view === "snapshot" ? "list" : state.view));
  }
  const view = { list: viewList, snapshot: viewSnapshot, recent: viewRecent }[state.view];
  try {
    main.replaceChildren(await view());
  } catch (e) {
    main.replaceChildren(e.reconnect ? reconnectView(e.message) : h("p", { class: "error" }, e.message));
  }
}

// Asks the running server to show this page with its key (POST /api/show needs none). The server
// points this tab, or a Dock tab, at the keyed link; the hashchange handler then re-renders.
function reconnectView(message) {
  const status = h("p", { class: "muted" });
  const button = h("button", { class: "primary", onclick: async () => {
    button.disabled = true;
    status.className = "muted";
    status.textContent = "Asking the ccsnap UI to show this page…";
    try {
      let r;
      try {
        r = await fetch("/api/show", { method: "POST", headers: { "Content-Type": "application/json" }, body: "{}" });
      } catch {
        throw new Error("The ccsnap UI server is not running.");
      }
      if (!r.ok) throw new Error(await r.text());
      const { shown } = await r.json();
      status.textContent = shown === "reused" ? "Reconnecting…" : `Opened the ccsnap UI in a new cmux ${shown === "dock" ? "Dock " : ""}tab.`;
    } catch (e) {
      status.className = "error";
      status.textContent = `${e.message} ${START_HINT}`;
    } finally {
      button.disabled = false;
    }
  } }, "Reconnect");
  return h("section", {}, h("p", { class: "error" }, message), h("div", { class: "row" }, button), status);
}

// ---- Snapshots ----

async function viewList() {
  const { result } = await ccsnap(["list"]);
  const name = h("input", { placeholder: "snapshot name" });
  const save = () => guard(async () => {
    const n = name.value.trim();
    if (!n) return toast("Give the snapshot a name.", "error");
    const r = await ccsnap(["save", n]);
    toast(r.exit === 3 ? `${n}: no changes` : `Saved ${n} v${r.result.version}`);
    go("snapshot", { snapshot: n, tab: "edit", version: null });
  });
  return h("section", {},
    h("div", { class: "row" }, name, h("button", { class: "primary", onclick: save }, "Save what is open")),
    result.snapshots.length === 0
      ? h("p", { class: "muted" }, "No snapshots yet.")
      : h("ul", { class: "cards" }, result.snapshots.map((s) =>
        h("li", { class: "card clickable", onclick: () => go("snapshot", { snapshot: s.name, tab: "edit", version: null }) },
          h("div", { class: "title" }, s.name, h("span", { class: "badge" }, `v${s.latestVersion}`)),
          h("div", { class: "muted" }, `${plural(s.workspaces, "workspace")} · ${plural(s.tabs, "tab")} · ${when(s.createdAt)}`),
          h("div", { class: "muted small" }, s.message)))));
}

async function viewSnapshot() {
  const name = state.snapshot;
  const body = await { edit: renderEdit, history: renderHistory, restore: renderRestore }[state.tab](name);
  const update = () => guard(async () => {
    if (opsOf(name).length) return toast("Save or discard your edits first.", "error");
    const r = await ccsnap(["save", name]);
    toast(r.exit === 3 ? "No changes since the last version." : `Saved v${r.result.version}: ${r.result.message}`);
    render();
  });
  const del = h("button", { class: "danger", onclick: () => guard(async () => {
    if (del.dataset.armed !== "1") {
      del.dataset.armed = "1";
      del.textContent = "Click again to delete";
      return;
    }
    await ccsnap(["delete", name, "--yes"]);
    pending.delete(name);
    toast(`Deleted ${name}`);
    go("list");
  }) }, "Delete");
  const labels = { edit: opsOf(name).length ? "Workspaces •" : "Workspaces", history: "History", restore: "Restore" };
  return h("section", {},
    h("div", { class: "row spread" },
      h("h2", {}, h("button", { class: "link", onclick: () => go("list") }, "Snapshots"), " / ", name),
      h("div", { class: "row" },
        h("button", { onclick: update, title: "Save what is open in cmux as a new version" }, "Update from open"), del)),
    h("div", { class: "tabs" }, Object.entries(labels).map(([key, label]) =>
      h("button", { class: key === state.tab ? "active" : "", onclick: () => go("snapshot", { tab: key, version: null }) }, label))),
    body);
}

// ---- Editor: ops are previewed with `edit --dry-run` and saved together as one version ----

async function renderEdit(name) {
  const ops = opsOf(name);
  let workspaces;
  let warnings;
  if (ops.length) {
    ({ workspaces, warnings } = (await ccsnap(["edit", name, "--batch", "-", "--dry-run"], JSON.stringify(ops))).result);
  } else {
    const r = await ccsnap(["show", name]);
    workspaces = r.result.version.workspaces;
    warnings = r.result.warnings;
  }
  return h("div", {},
    warnings.length ? h("ul", { class: "warnings" }, warnings.map((w) => h("li", {}, w))) : null,
    workspaces.map((w, wi) => editWorkspace(w, wi, workspaces.length)),
    addWorkspaceCard(name),
    saveBar(name));
}

// Adds ops for the open snapshot; keeps them only if ccsnap accepts them.
async function pushOp(...ops) {
  const list = opsOf(state.snapshot);
  list.push(...ops);
  try {
    await ccsnap(["edit", state.snapshot, "--batch", "-", "--dry-run"], JSON.stringify(list));
  } catch (e) {
    list.splice(list.length - ops.length, ops.length);
    toast(e.message, "error");
    return;
  }
  render();
}

// Text that becomes an input on double-click; calls onsave(text) on Enter or when focus leaves.
function editable(text, onsave) {
  const span = h("span", { class: "editable", title: "Double-click to rename" }, text);
  span.addEventListener("dblclick", () => {
    const input = h("input", { value: text });
    let done = false;
    const finish = (save) => {
      if (done) return;
      done = true;
      const value = input.value.trim();
      if (save && value && value !== text) onsave(value);
      else input.replaceWith(span);
    };
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") finish(true);
      if (e.key === "Escape") finish(false);
    });
    input.addEventListener("blur", () => finish(true));
    span.replaceWith(input);
    input.focus();
    input.select();
  });
  return span;
}

function editWorkspace(w, wi, count) {
  const card = h("div", { class: "card" },
    h("div", { class: "title" },
      editable(w.name, (to) => pushOp({ op: "ws-rename", ws: w.name, to })),
      h("span", { class: "muted small" }, w.cwd),
      h("span", { class: "row" },
        h("button", { class: "icon", title: "Move up", disabled: wi === 0, onclick: () => pushOp({ op: "ws-index", ws: w.name, index: wi }) }, "↑"),
        h("button", { class: "icon", title: "Move down", disabled: wi === count - 1, onclick: () => pushOp({ op: "ws-index", ws: w.name, index: wi + 2 }) }, "↓"),
        h("button", { class: "icon", title: "Remove workspace", onclick: () => pushOp({ op: "ws-remove", ws: w.name }) }, "×"))),
    h("ol", { class: "tabs-list" }, w.tabs.map((t, ti) => editTab(w, t, ti + 1))),
    addTabRow(w));
  card.addEventListener("dragover", (e) => {
    if (!dragged) return;
    e.preventDefault();
    card.classList.add("drop-target");
  });
  card.addEventListener("dragleave", () => card.classList.remove("drop-target"));
  card.addEventListener("drop", (e) => {
    e.preventDefault();
    card.classList.remove("drop-target");
    if (dragged && dragged.ws !== w.name) pushOp({ op: "tab-move", tab: `${dragged.ws}#${dragged.pos}`, to: w.name });
  });
  return card;
}

function editTab(w, t, pos) {
  const ref = `${w.name}#${pos}`;
  const li = h("li", { draggable: "true" },
    h("span", { class: `badge ${t.type}` }, tabLabel(t)), " ",
    editable(t.title, (to) => pushOp({ op: "tab-rename", tab: ref, to })),
    h("span", { class: "muted small" }, ` ${t.cwd}`),
    h("button", { class: "icon", title: "Remove tab", onclick: () => pushOp({ op: "tab-remove", tab: ref }) }, "×"));
  li.addEventListener("dragstart", (e) => {
    dragged = { ws: w.name, pos };
    e.dataTransfer.setData("text/plain", ref);
    li.classList.add("dragging");
  });
  li.addEventListener("dragend", () => {
    dragged = null;
    li.classList.remove("dragging");
  });
  li.addEventListener("dragover", (e) => {
    if (!dragged) return;
    e.preventDefault();
    e.stopPropagation();
    li.classList.add("drop-target");
  });
  li.addEventListener("dragleave", () => li.classList.remove("drop-target"));
  li.addEventListener("drop", (e) => {
    e.preventDefault();
    e.stopPropagation();
    li.classList.remove("drop-target");
    const from = dragged;
    if (!from) return;
    if (from.ws === w.name) {
      if (from.pos !== pos) pushOp({ op: "tab-index", tab: `${from.ws}#${from.pos}`, index: pos });
    } else {
      // tab-move appends to the target workspace; then move it to where it was dropped.
      pushOp(
        { op: "tab-move", tab: `${from.ws}#${from.pos}`, to: w.name },
        { op: "tab-index", tab: `${w.name}#${w.tabs.length + 1}`, index: pos },
      );
    }
  });
  return li;
}

// "+ Tab": search Claude sessions and add one, or add a shell in the workspace's folder.
function addTabRow(w) {
  const results = h("div", { class: "picker" });
  const search = h("input", { placeholder: "+ Claude session: search" });
  search.addEventListener("input", () => {
    clearTimeout(search.timer);
    search.timer = setTimeout(() => guard(async () => {
      const q = search.value.trim();
      if (!q) return results.replaceChildren();
      const r = await ccsnap(["sessions", "--search", q, "--days", "365"]);
      results.replaceChildren(h("ul", {}, r.result.sessions.slice(0, 20).map(({ session: s, open }) =>
        h("li", { onclick: () => pushOp({ op: "tab-add-session", ws: w.name, sessionId: s.sessionId }) },
          s.title || s.firstPrompt || s.sessionId,
          h("span", { class: "muted small" }, ` ${s.cwd || ""}${open ? " · open in cmux" : ""}`)))));
    }), 300);
  });
  return h("div", {},
    h("div", { class: "row" }, search,
      h("button", { onclick: () => pushOp({ op: "tab-add-terminal", ws: w.name, cwd: w.cwd }) }, "+ Shell")),
    results);
}

// "+ Workspace": one open in cmux now, or one copied from another snapshot.
function addWorkspaceCard(name) {
  const out = h("div", { class: "picker" });
  const fromOpen = () => guard(async () => {
    const r = await ccsnap(["open"]);
    out.replaceChildren(h("ul", {}, r.result.workspaces.map((w) =>
      h("li", { onclick: () => pushOp({ op: "ws-add-live", ws: w.name }) },
        w.name, h("span", { class: "muted small" }, ` ${plural(w.tabs.length, "tab")}`)))));
  });
  const fromSnapshot = () => guard(async () => {
    const r = await ccsnap(["list"]);
    out.replaceChildren(h("ul", {}, r.result.snapshots.filter((s) => s.name !== name).map((s) =>
      h("li", { onclick: () => guard(async () => {
        const v = await ccsnap(["show", s.name]);
        out.replaceChildren(h("ul", {}, v.result.version.workspaces.map((w) =>
          h("li", { onclick: () => pushOp({ op: "ws-from", from: s.name, ws: w.name }) }, `${s.name} › ${w.name}`))));
      }) }, s.name))));
  });
  return h("div", { class: "card" },
    h("div", { class: "row" }, h("span", { class: "muted" }, "+ Workspace"),
      h("button", { onclick: fromOpen }, "from cmux"),
      h("button", { onclick: fromSnapshot }, "from another snapshot")),
    out);
}

function saveBar(name) {
  const ops = opsOf(name);
  if (ops.length === 0) {
    return h("p", { class: "muted" }, "Double-click a name to rename it. Drag tabs to reorder them or move them to another workspace.");
  }
  const message = h("input", { placeholder: "version message (optional)" });
  const save = () => guard(async () => {
    const args = ["edit", name, "--batch", "-"];
    if (message.value.trim()) args.push("-m", message.value.trim());
    const r = await ccsnap(args, JSON.stringify(ops));
    pending.delete(name);
    toast(r.exit === 3 ? "No changes to save." : `Saved v${r.result.version}: ${r.result.message}`);
    render();
  });
  const discard = () => {
    pending.delete(name);
    render();
  };
  return h("div", { class: "savebar row" },
    h("strong", {}, plural(ops.length, "unsaved change")), message,
    h("button", { class: "primary", onclick: save }, "Save as new version"),
    h("button", { onclick: discard }, "Discard"));
}

// ---- History ----

function describeChange(c) {
  switch (c.change) {
    case "workspace-added": return `+ workspace ${c.workspace}`;
    case "workspace-removed": return `− workspace ${c.workspace}`;
    case "tab-added": return `+ ${c.workspace} › ${c.title}`;
    case "tab-removed": return `− ${c.workspace} › ${c.title}`;
    case "tab-moved": return `↷ ${c.title}: ${c.from} → ${c.to}`;
    case "tab-renamed": return `✎ ${c.workspace}: ${c.from} → ${c.to}`;
    default: return JSON.stringify(c);
  }
}

async function renderHistory(name) {
  const { result } = await ccsnap(["history", name]);
  const picked = new Set();
  const out = h("div", {});
  const compare = h("button", { disabled: true, onclick: () => guard(async () => {
    const [a, b] = [...picked].sort((x, y) => x - y);
    const d = await ccsnap(["diff", `${name}@${a}`, `${name}@${b}`]);
    out.replaceChildren(h("h3", {}, `v${a} → v${b}: ${d.result.summary}`),
      h("ul", {}, d.result.changes.map((c) => h("li", {}, describeChange(c)))));
  }) }, "Compare");
  return h("div", {},
    h("p", { class: "muted" }, "Tick two versions to compare them."),
    h("table", { class: "versions" }, [...result.versions].reverse().map((v) => h("tr", {},
      h("td", {}, h("input", { type: "checkbox", onchange: (e) => {
        if (e.target.checked) picked.add(v.version);
        else picked.delete(v.version);
        compare.disabled = picked.size !== 2;
      } })),
      h("td", {}, `v${v.version}`),
      h("td", {}, when(v.createdAt)),
      h("td", {}, v.message),
      h("td", { class: "muted" }, `${plural(v.workspaces, "workspace")}, ${plural(v.tabs, "tab")}`),
      h("td", {}, h("button", { class: "link", onclick: () => go("snapshot", { tab: "restore", version: v.version }) }, "Restore…"))))),
    compare, out);
}

// ---- Restore ----

async function renderRestore(name) {
  const ref = state.version ? `${name}@${state.version}` : name;
  const { result } = await ccsnap(["show", ref]);
  const boxes = []; // { ws, pos, box }
  const out = h("div", {});
  const tree = result.version.workspaces.map((w) => {
    const wsBox = h("input", { type: "checkbox", checked: true });
    const tabBoxes = w.tabs.map((t, i) => {
      const box = h("input", { type: "checkbox", checked: true });
      boxes.push({ ws: w.name, pos: i + 1, box });
      box.addEventListener("change", () => {
        wsBox.checked = tabBoxes.every((b) => b.checked);
        wsBox.indeterminate = !wsBox.checked && tabBoxes.some((b) => b.checked);
      });
      return box;
    });
    wsBox.addEventListener("change", () => {
      for (const b of tabBoxes) b.checked = wsBox.checked;
      wsBox.indeterminate = false;
    });
    return h("li", {},
      h("label", { class: "title" }, wsBox, " ", w.name),
      h("ul", {}, w.tabs.map((t, i) => h("li", {}, h("label", {}, tabBoxes[i], " ", t.title)))));
  });
  const makePlan = () => guard(async () => {
    const chosen = boxes.filter((b) => b.box.checked);
    if (chosen.length === 0) return toast("Tick at least one tab.", "error");
    const args = ["restore", ref, "--plan"];
    if (chosen.length < boxes.length) {
      for (const b of chosen) args.push("--tab", `${b.ws}#${b.pos}`);
    }
    out.replaceChildren(renderPlan(await ccsnap(args)));
  });
  return h("div", {},
    state.version ? h("p", { class: "muted" }, `Restoring version ${state.version}.`) : null,
    h("ul", { class: "tree" }, tree),
    h("button", { class: "primary", onclick: makePlan }, "Show what would open"),
    out);
}

function renderPlan(r) {
  const plan = r.result.plan;
  if (r.exit === 3 || plan.actions.length === 0) {
    return h("p", { class: "ok" }, "Nothing to restore: everything is already open.");
  }
  const apply = h("button", { class: "primary", onclick: () => guard(async () => {
    apply.disabled = true;
    try {
      const a = await ccsnap(["apply", plan.planId]);
      apply.replaceWith(renderSteps(a.result));
    } catch (e) {
      apply.disabled = false;
      throw e;
    }
  }) }, "Restore now");
  return h("div", { class: "plan" },
    h("ul", {}, plan.actions.map((a) => h("li", {},
      a.action === "create-workspace"
        ? `+ create workspace ${a.name} (${plural(a.tabs.length, "tab")})`
        : `~ add to ${a.workspaceName}`,
      h("ul", {}, a.tabs.map((t) => h("li", {}, t.title)))))),
    plan.skipped.length
      ? h("details", {}, h("summary", {}, `${plan.skipped.length} already open or skipped`),
        h("ul", {}, plan.skipped.map((s) => h("li", {}, s))))
      : null,
    plan.warnings.length ? h("ul", { class: "warnings" }, plan.warnings.map((w) => h("li", {}, w))) : null,
    apply);
}

function renderSteps(result) {
  return h("div", {},
    h("p", { class: result.failed ? "error" : "ok" }, result.failed ? `${plural(result.failed, "step")} failed.` : "Restored."),
    h("ul", {}, result.steps.map((s) =>
      h("li", { class: s.ok ? "" : "error" }, s.ok ? "✓ " : "✗ ", s.description, s.error ? `: ${s.error}` : ""))));
}

// ---- Recent ----

async function viewRecent() {
  const hours = state.hours;
  const r = await ccsnap(["recent", "--hours", String(hours)]);
  const out = h("div", {});
  const pick = h("select", { onchange: (e) => go("recent", { hours: Number(e.target.value) }) },
    [24, 72, 168].map((n) => h("option", { value: n, selected: n === hours }, n === 24 ? "Last 24 hours" : `Last ${n / 24} days`)));
  const sessions = r.result.sessions;
  return h("section", {},
    h("div", { class: "row spread" }, h("h2", {}, "Claude sessions not open in cmux"), pick),
    sessions.length === 0
      ? h("p", { class: "ok" }, "Nothing missing.")
      : h("div", {},
        h("ul", { class: "cards" }, sessions.map((s) => h("li", { class: "card" },
          h("div", { class: "title" }, s.title || s.firstPrompt || s.sessionId),
          h("div", { class: "muted small" }, `${s.cwd} · ${when(s.modified)}`)))),
        h("button", { class: "primary", onclick: () => guard(async () => {
          out.replaceChildren(renderPlan(await ccsnap(["recent", "--hours", String(hours), "--plan"])));
        }) }, "Show what would open"),
        out));
}

for (const b of document.querySelectorAll("nav button")) b.addEventListener("click", () => go(b.dataset.view));
// `ccsnap ui` and Reconnect point an open tab at the key by changing only the fragment.
window.addEventListener("hashchange", () => {
  TOKEN = takeToken();
  render();
});
render();
