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
const expanded = new Set(); // "snapshot/workspace" keys whose tabs are shown
let dragged = null; // the tab being dragged: { ws, pos }
const opLabels = new WeakMap(); // edit op → what it does, in words, for the save bar
// Returns `op` after noting how to describe it.
function said(op, text) {
  opLabels.set(op, text);
  return op;
}

// `reconnect`: the page has no usable key; render() then offers the Reconnect button.
class CommandError extends Error {
  constructor(message, exit, reconnect = false) {
    super(message);
    this.exit = exit;
    this.reconnect = reconnect;
  }
}

const START_HINT = "Start it with the palette command \"ccsnap: Open UI\" or `ccsnap ui`.";

// While any command runs, a bar slides under the header.
let running = 0;
function setBusy(delta) {
  running += delta;
  document.body.classList.toggle("busy", running > 0);
}

// Runs `ccsnap <args> --json`. Resolves to { exit, code, result }; rejects on errors.
async function ccsnap(args, input) {
  if (!TOKEN) throw new CommandError("This page has no key yet.", 1, true);
  setBusy(1);
  try {
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
  } finally {
    setBusy(-1);
  }
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

// Small stroke icons (paths on a 16×16 grid).
const ICONS = {
  chevron: "M6 3.5 10.5 8 6 12.5",
  chevdown: "M3.5 6 8 10.5 12.5 6",
  back: "M10 3.5 5.5 8 10 12.5",
  close: "M4.5 4.5l7 7M11.5 4.5l-7 7",
  up: "M8 12.5v-9M4.5 7 8 3.5 11.5 7",
  down: "M8 3.5v9M4.5 9 8 12.5 11.5 9",
  pencil: "M10.5 3.5l2 2L6 12H4v-2z",
  grip: "M6 4h.01M10 4h.01M6 8h.01M10 8h.01M6 12h.01M10 12h.01",
  refresh: "M13 8a5 5 0 1 1-1.5-3.5M13 3v2.5h-2.5",
  terminal: "M3 4.5 6.5 8 3 11.5M8 11.5h5",
  claude: "M8 2v12M2 8h12M3.8 3.8l8.4 8.4M12.2 3.8l-8.4 8.4",
  plus: "M8 3.5v9M3.5 8h9",
  trash: "M3.5 4.5h9M6.5 4.5V3h3v1.5M5 4.5l.5 8h5l.5-8",
  check: "M3.5 8.5 6.5 11.5 12.5 4.5",
  warn: "M8 2.5 14 13H2zM8 6.5v3M8 11.2h.01",
  branch: "M5 2.5v11M11 6.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3zM11 6.5c0 3.5-6 2.5-6 5.5",
  plug: "M6 2.5v3M10 2.5v3M4.5 5.5h7v2a3.5 3.5 0 0 1-7 0zM8 11v2.5",
};
function icon(name, cls = "") {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", name === "grip" ? "2.4" : "1.6");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  if (cls) svg.setAttribute("class", cls);
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", ICONS[name]);
  svg.append(path);
  return svg;
}
function iconButton(name, title, onclick, cls = "") {
  return h("button", { class: `icon ${cls}`, title, "aria-label": title, onclick }, icon(name));
}

function toast(message, kind = "info") {
  const t = document.getElementById("toast");
  t.textContent = message;
  t.className = `toast ${kind}`;
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => { t.className = `toast ${kind} hidden`; }, kind === "error" ? 7000 : 3500);
}

// Runs an action and shows its error instead of throwing.
async function guard(fn) {
  try {
    await fn();
  } catch (e) {
    toast(e.message, "error");
  }
}

// A button that shows a spinner and ignores clicks while its action runs.
function actionButton(label, fn, attrs = {}) {
  const b = h("button", attrs, label);
  b.addEventListener("click", async () => {
    if (b.classList.contains("busy")) return;
    b.classList.add("busy");
    b.disabled = true;
    try {
      await guard(fn);
    } finally {
      b.classList.remove("busy");
      b.disabled = false;
    }
  });
  return b;
}

function when(iso) { return iso ? new Date(iso).toLocaleString() : ""; }
// "just now", "5m ago", "3h ago", "2d ago", else a short date.
function ago(iso) {
  if (!iso) return "";
  const t = new Date(iso);
  const s = (Date.now() - t.getTime()) / 1000;
  if (s < 45) return "just now";
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  if (s < 86400) return `${Math.round(s / 3600)}h ago`;
  if (s < 7 * 86400) return `${Math.round(s / 86400)}d ago`;
  return t.toLocaleDateString(undefined, { month: "short", day: "numeric", year: t.getFullYear() === new Date().getFullYear() ? undefined : "numeric" });
}
function time(iso) { return h("span", { title: when(iso) }, ago(iso)); }
function plural(n, word) { return `${n} ${word}${n === 1 ? "" : "s"}`; }
// "/Users/me/x" → "~/x"
function tilde(p) { return (p || "").replace(/^\/Users\/[^/]+(?=\/|$)/, "~"); }
// A tab's folder relative to its workspace's: "" when the same.
function relPath(cwd, base) {
  if (!cwd || cwd === base) return "";
  const rel = base && cwd.startsWith(`${base}/`) ? cwd.slice(base.length + 1) : tilde(cwd);
  const tree = rel.match(/(?:^|\/)\.claude\/worktrees\/([^/]+)$/);
  return tree ? `worktree · ${tree[1]}` : rel;
}
function opsOf(name) {
  if (!pending.has(name)) pending.set(name, []);
  return pending.get(name);
}
function kindOf(t) { return t.type === "claude" ? "Claude session" : t.command ? `Runs: ${t.command}` : "Shell"; }
function kindIcon(t) {
  return h("span", { class: `kind ${t.type === "claude" ? "claude" : ""}`, title: kindOf(t) }, icon(t.type === "claude" ? "claude" : "terminal"));
}

function go(view, extra = {}) {
  Object.assign(state, { view }, extra);
  render();
}

let renderSeq = 0;
async function render() {
  const seq = ++renderSeq;
  for (const b of document.querySelectorAll("nav button")) {
    b.classList.toggle("active", b.dataset.view === (state.view === "snapshot" ? "list" : state.view));
  }
  const view = { list: viewList, snapshot: viewSnapshot, recent: viewRecent }[state.view];
  let content;
  try {
    content = await view();
  } catch (e) {
    content = e.reconnect ? reconnectView(e.message) : errorView(e.message);
  }
  // A newer render started meanwhile: its result wins.
  if (seq === renderSeq) main.replaceChildren(content);
}

function errorView(message) {
  return h("div", { class: "status" },
    icon("warn", "icon-lg"),
    h("p", { class: "error" }, message),
    h("button", { onclick: render }, "Try again"));
}

// Asks the running server to show this page with its key (POST /api/show needs none). The server
// points this tab, or a Dock tab, at the keyed link; the hashchange handler then re-renders.
function reconnectView(message) {
  const status = h("p", { class: "muted small" });
  const button = actionButton("Reconnect", async () => {
    status.className = "muted small";
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
      status.className = "error small";
      status.textContent = `${e.message} ${START_HINT}`;
    }
  }, { class: "primary" });
  return h("div", { class: "status" },
    icon("plug", "icon-lg"),
    h("h2", {}, "Not connected"),
    h("p", { class: "muted" }, message),
    button,
    status);
}

// ---- Snapshots ----

async function viewList() {
  const { result } = await ccsnap(["list"]);
  const name = h("input", { class: "grow", placeholder: "Name, e.g. main", "aria-label": "Snapshot name" });
  const save = actionButton("Save open tabs", async () => {
    const n = name.value.trim();
    if (!n) {
      name.focus();
      return toast("Give the snapshot a name.", "error");
    }
    const r = await ccsnap(["save", n]);
    toast(r.exit === 3 ? `${n}: nothing changed since the last version` : `Saved ${n} v${r.result.version}`);
    go("snapshot", { snapshot: n, tab: "edit", version: null });
  }, { class: "primary" });
  name.addEventListener("keydown", (e) => { if (e.key === "Enter") save.click(); });
  const open = (s) => go("snapshot", { snapshot: s.name, tab: "edit", version: null });
  return h("section", { class: "stack" },
    h("div", { class: "row" }, name, save),
    result.snapshots.length === 0
      ? h("div", { class: "empty" }, h("strong", {}, "No snapshots yet"),
        "Name one above to save the workspaces and tabs open in cmux now.")
      : h("ul", { class: "list" }, result.snapshots.map((s) => {
        const pend = opsOf(s.name).length;
        const li = h("li", { class: "item clickable", tabindex: "0", role: "button", onclick: () => open(s),
          onkeydown: (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); open(s); } } },
        h("div", { class: "grow" },
          h("div", { class: "meta" },
            h("strong", { class: "ellipsis" }, s.name),
            h("span", { class: "pill" }, `v${s.latestVersion}`),
            pend ? h("span", { class: "pill accent" }, `${pend} unsaved`) : null,
            h("span", { class: "grow" }),
            h("span", { class: "muted small" }, time(s.createdAt))),
          h("div", { class: "muted small" }, `${plural(s.workspaces, "workspace")} · ${plural(s.tabs, "tab")}`),
          s.message ? h("div", { class: "faint small ellipsis" }, s.message) : null),
        icon("chevron", "chev"));
        return li;
      })));
}

async function viewSnapshot() {
  const name = state.snapshot;
  // The header and tabs come first, so a body that fails still leaves History, Restore and Discard.
  const body = h("div", {});
  const update = actionButton("Update", async () => {
    if (opsOf(name).length) return toast("Save or discard your edits first.", "error");
    const r = await ccsnap(["save", name]);
    toast(r.exit === 3 ? "Nothing changed since the last version." : `Saved v${r.result.version}: ${r.result.message}`);
    render();
  }, { title: "Save what is open in cmux now as a new version" });
  const del = h("button", { class: "icon remove", title: "Delete snapshot", "aria-label": "Delete snapshot" }, icon("trash"));
  del.addEventListener("click", () => guard(async () => {
    if (!del.classList.contains("armed")) {
      del.classList.add("armed");
      del.classList.remove("icon");
      del.classList.add("danger");
      del.replaceChildren(`Delete ${name}?`);
      clearTimeout(del.timer);
      del.timer = setTimeout(() => {
        del.classList.remove("armed", "danger");
        del.classList.add("icon");
        del.replaceChildren(icon("trash"));
      }, 4000);
      return;
    }
    clearTimeout(del.timer);
    await ccsnap(["delete", name, "--yes"]);
    pending.delete(name);
    toast(`Deleted ${name}`);
    go("list");
  }));
  const dirty = opsOf(name).length > 0;
  const tabs = h("div", { class: "seg full", role: "tablist" },
    [["edit", "Workspaces"], ["history", "History"], ["restore", "Restore"]].map(([key, label]) =>
      h("button", { class: key === state.tab ? "active" : "", role: "tab", "aria-selected": String(key === state.tab),
        onclick: () => go("snapshot", { tab: key, version: null }) },
      label, key === "edit" && dirty ? h("span", { class: "dot", title: "Unsaved edits" }) : null)));
  const renderBody = { edit: renderEdit, history: renderHistory, restore: renderRestore }[state.tab];
  try {
    body.replaceChildren(await renderBody(name));
  } catch (e) {
    if (e.reconnect) throw e;
    body.replaceChildren(h("div", { class: "banner warn" }, icon("warn"), h("span", {}, e.message)));
  }
  return h("section", { class: "stack" },
    h("div", {},
      h("div", { class: "crumb" }, h("button", { class: "link", onclick: () => go("list") }, icon("back"), "Snapshots")),
      h("div", { class: "page-title" }, h("h1", {}, name), h("span", { class: "grow" }), update, del)),
    tabs,
    body);
}

// ---- Editor: ops are previewed with `edit --dry-run` and saved together as one version ----

async function renderEdit(name) {
  const ops = opsOf(name);
  let workspaces;
  let warnings;
  if (ops.length) {
    try {
      ({ workspaces, warnings } = (await ccsnap(["edit", name, "--batch", "-", "--dry-run"], JSON.stringify(ops))).result);
    } catch (e) {
      if (e.reconnect) throw e;
      // The snapshot changed outside this page and the queued edits no longer apply.
      return h("div", { class: "stack" },
        h("div", { class: "banner warn" }, icon("warn"), h("span", {}, `Your unsaved edits no longer apply: ${e.message}`)),
        h("button", { onclick: () => { pending.delete(name); render(); } }, "Discard edits"));
    }
  } else {
    const r = await ccsnap(["show", name]);
    workspaces = r.result.version.workspaces;
    warnings = r.result.warnings;
  }
  const keys = workspaces.map((w) => `${name}/${w.name}`);
  // Small snapshots start open.
  if (workspaces.length <= 2 && !keys.some((k) => expanded.has(k))) keys.forEach((k) => expanded.add(k));
  const allOpen = keys.length > 0 && keys.every((k) => expanded.has(k));
  const tabCount = workspaces.reduce((n, w) => n + w.tabs.length, 0);
  return h("div", {},
    warnings.length ? h("ul", { class: "banner warn warnings" }, warnings.map((w) => h("li", {}, w))) : null,
    h("div", { class: "toolbar" },
      h("span", { class: "muted small" }, `${plural(workspaces.length, "workspace")} · ${plural(tabCount, "tab")}`),
      h("button", { class: "link small", onclick: () => {
        for (const k of keys) allOpen ? expanded.delete(k) : expanded.add(k);
        render();
      } }, allOpen ? "Collapse all" : "Expand all")),
    h("div", { class: "hint" }, "Double-click a name to rename · drag tabs to reorder or move"),
    h("div", { class: "ws-list" }, workspaces.map((w, wi) => editWorkspace(name, w, wi, workspaces.length))),
    addWorkspace(name, new Set(workspaces.map((w) => w.name))),
    saveBar(name));
}

// Adds ops for the open snapshot; keeps them only if ccsnap accepts them.
const opGroups = new WeakMap(); // op → the ops added with it, which Undo removes together
async function pushOp(...ops) {
  const list = opsOf(state.snapshot);
  for (const op of ops) opGroups.set(op, ops);
  list.push(...ops);
  try {
    await ccsnap(["edit", state.snapshot, "--batch", "-", "--dry-run"], JSON.stringify(list));
  } catch (e) {
    list.splice(list.length - ops.length, ops.length);
    toast(e.message, "error");
    // Re-render so a rename input that was refused goes back to its name.
  }
  render();
}

// Text that becomes an input on double-click (or via .startEdit()); calls onsave(text) on Enter
// or when focus leaves, and puts the text back on Escape.
function editable(text, onsave, cls = "") {
  const span = h("span", { class: `editable ellipsis ${cls}`, title: `${text} (double-click to rename)` }, text);
  span.startEdit = () => {
    const input = h("input", { class: "rename", value: text, "aria-label": "New name" });
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
    input.addEventListener("click", (e) => e.stopPropagation());
    span.replaceWith(input);
    input.focus();
    input.select();
  };
  span.addEventListener("dblclick", (e) => {
    e.stopPropagation();
    span.startEdit();
  });
  return span;
}

function editWorkspace(name, w, wi, count) {
  const key = `${name}/${w.name}`;
  const open = expanded.has(key);
  const toggle = () => {
    open ? expanded.delete(key) : expanded.add(key);
    render();
  };
  const title = editable(w.name, (to) => {
    if (expanded.delete(key)) expanded.add(`${name}/${to}`);
    pushOp(said({ op: "ws-rename", ws: w.name, to }, `Renamed workspace ${w.name} to ${to}`));
  }, "name");
  const head = h("div", { class: "ws-head" },
    h("button", { class: `icon toggle ${open ? "open" : ""}`, title: open ? "Hide tabs" : "Show tabs",
      "aria-label": open ? "Hide tabs" : "Show tabs", "aria-expanded": String(open), onclick: toggle }, icon("chevron")),
    h("div", { class: "titles" },
      h("div", { class: "row" }, title, h("span", { class: "pill" }, w.tabs.length)),
      h("div", { class: "sub" },
        h("span", { class: "mono muted ellipsis", title: w.cwd }, tilde(w.cwd)),
        w.gitBranch ? h("span", { class: "branch mono faint ellipsis", title: `git branch ${w.gitBranch}` }, icon("branch"), w.gitBranch) : null)),
    h("div", { class: "actions" },
      iconButton("pencil", "Rename workspace", () => title.startEdit()),
      h("button", { class: "icon", title: "Move up", "aria-label": "Move up", disabled: wi === 0,
        onclick: () => pushOp(said({ op: "ws-index", ws: w.name, index: wi }, `Moved workspace ${w.name} up`)) }, icon("up")),
      h("button", { class: "icon", title: "Move down", "aria-label": "Move down", disabled: wi === count - 1,
        onclick: () => pushOp(said({ op: "ws-index", ws: w.name, index: wi + 2 }, `Moved workspace ${w.name} down`)) }, icon("down")),
      iconButton("close", "Remove workspace from the snapshot", () => pushOp(said({ op: "ws-remove", ws: w.name }, `Removed workspace ${w.name}`)), "remove")));
  head.addEventListener("click", (e) => {
    if (e.target === head || e.target.closest(".titles") && !e.target.closest(".editable, input")) toggle();
  });
  const card = h("div", { class: "ws" }, head,
    open ? h("div", { class: "ws-body" },
      h("ol", {}, w.tabs.map((t, ti) => editTab(w, t, ti + 1))),
      addTabRow(w)) : null);
  card.addEventListener("dragover", (e) => {
    if (!dragged) return;
    e.preventDefault();
    card.classList.add("drop-target");
  });
  card.addEventListener("dragleave", (e) => {
    if (!card.contains(e.relatedTarget)) card.classList.remove("drop-target");
  });
  card.addEventListener("drop", (e) => {
    e.preventDefault();
    card.classList.remove("drop-target");
    if (dragged && dragged.ws !== w.name) {
      pushOp(said({ op: "tab-move", tab: `${dragged.ws}#${dragged.pos}`, to: w.name }, `Moved ${dragged.title} to ${w.name}`));
    }
  });
  return card;
}

function editTab(w, t, pos) {
  const ref = `${w.name}#${pos}`;
  const title = editable(t.title, (to) => pushOp(said({ op: "tab-rename", tab: ref, to }, `Renamed tab ${t.title} to ${to}`)));
  const where = relPath(t.cwd, w.cwd);
  const li = h("li", { class: "tab", draggable: "true" },
    h("span", { class: "grip", title: "Drag to move" }, icon("grip")),
    kindIcon(t),
    h("div", { class: "title" }, title,
      where ? h("span", { class: "mono faint ellipsis", title: t.cwd }, where) : null),
    h("div", { class: "actions" },
      iconButton("pencil", "Rename tab", () => title.startEdit()),
      iconButton("close", "Remove tab from the snapshot", () => pushOp(said({ op: "tab-remove", tab: ref }, `Removed tab ${t.title}`)), "remove")));
  li.addEventListener("dragstart", (e) => {
    dragged = { ws: w.name, pos, title: t.title };
    e.dataTransfer.setData("text/plain", ref);
    e.dataTransfer.effectAllowed = "move";
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
    li.closest(".ws")?.classList.remove("drop-target");
    const from = dragged;
    if (!from) return;
    if (from.ws === w.name) {
      if (from.pos !== pos) pushOp(said({ op: "tab-index", tab: `${from.ws}#${from.pos}`, index: pos }, `Moved ${from.title} to position ${pos}`));
    } else {
      // tab-move appends to the target workspace; then move it to where it was dropped.
      pushOp(
        said({ op: "tab-move", tab: `${from.ws}#${from.pos}`, to: w.name }, `Moved ${from.title} to ${w.name}`),
        { op: "tab-index", tab: `${w.name}#${w.tabs.length + 1}`, index: pos },
      );
    }
  });
  return li;
}

// A search box over a list: `load(query)` returns [{ label, sub, pick }].
function picker(placeholder, load, { immediate = false, emptyNote = "Nothing to add." } = {}) {
  const results = h("ul", {});
  const input = h("input", { placeholder, "aria-label": placeholder });
  const show = (items) => {
    results.replaceChildren(...(items.length
      ? items.map((it) => h("li", { tabindex: "0", onclick: it.pick,
        onkeydown: (e) => { if (e.key === "Enter") it.pick(); } },
      h("span", { class: "ellipsis" }, it.label),
      it.sub ? h("span", { class: "muted small ellipsis" }, it.sub) : null))
      : [h("li", { class: "note" }, input.value.trim() ? "No matches." : emptyNote)]));
  };
  // `load` returns null for "nothing typed yet": the list then stays empty.
  const run = () => guard(async () => {
    const items = await load(input.value.trim());
    if (items) show(items);
    else results.replaceChildren();
  });
  input.addEventListener("input", () => {
    clearTimeout(input.timer);
    input.timer = setTimeout(run, 250);
  });
  input.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") results.querySelector("li[tabindex]")?.focus();
  });
  const el = h("div", { class: "picker" }, input, results);
  el.focusInput = () => input.focus();
  if (immediate) run();
  return el;
}

// "+ Claude session" (search) or "+ Shell" in the workspace's folder.
function addTabRow(w) {
  const slot = h("div", {});
  const sessions = h("button", { class: "ghost", onclick: () => {
    if (slot.firstChild) return slot.replaceChildren();
    const p = picker("Search Claude sessions by title, prompt or folder", async (q) => {
      if (!q) return null;
      const r = await ccsnap(["sessions", "--search", q, "--days", "365"]);
      return r.result.sessions.slice(0, 30).map(({ session: s, open }) => ({
        label: s.title || s.firstPrompt || s.sessionId,
        sub: `${tilde(s.cwd)}${open ? " · open in cmux" : ""}`,
        pick: () => pushOp(said({ op: "tab-add-session", ws: w.name, sessionId: s.sessionId }, `Added ${s.title || "a Claude session"} to ${w.name}`)),
      }));
    });
    slot.replaceChildren(p);
    p.focusInput();
  } }, icon("plus"), "Claude session");
  const shell = h("button", { class: "ghost", onclick: () => pushOp(said({ op: "tab-add-terminal", ws: w.name, cwd: w.cwd }, `Added a shell to ${w.name}`)) },
    icon("plus"), "Shell");
  return h("div", {}, h("div", { class: "add-row" }, sessions, shell), slot);
}

// "+ Add workspace": one open in cmux now, or one copied from another snapshot.
function addWorkspace(name, have) {
  const wrap = h("div", {});
  const closed = () => h("button", { class: "add-ws", onclick: () => wrap.replaceChildren(panel()) }, icon("plus"), "Add workspace");
  const panel = () => {
    const slot = h("div", {});
    const fromOpen = () => {
      const p = picker("Filter workspaces open in cmux", async (q) => {
        const r = await ccsnap(["open"]);
        return r.result.workspaces
          .filter((w) => !have.has(w.name))
          .filter((w) => !q || w.name.toLowerCase().includes(q.toLowerCase()))
          .map((w) => ({ label: w.name, sub: plural(w.tabs.length, "tab"), pick: () => pushOp(said({ op: "ws-add-live", ws: w.name }, `Added workspace ${w.name} from cmux`)) }));
      }, { immediate: true, emptyNote: "Every workspace open in cmux is already in this snapshot." });
      slot.replaceChildren(p);
      p.focusInput();
    };
    const fromSnapshot = () => {
      const p = picker("Filter workspaces in other snapshots", async (q) => {
        const r = await ccsnap(["list"]);
        const items = [];
        for (const s of r.result.snapshots.filter((x) => x.name !== name)) {
          const v = await ccsnap(["show", s.name]);
          for (const w of v.result.version.workspaces) {
            if (have.has(w.name)) continue;
            if (q && !`${s.name} ${w.name}`.toLowerCase().includes(q.toLowerCase())) continue;
            items.push({ label: w.name, sub: `from ${s.name} · ${plural(w.tabs.length, "tab")}`,
              pick: () => pushOp(said({ op: "ws-from", from: s.name, ws: w.name }, `Added workspace ${w.name} from ${s.name}`)) });
          }
        }
        return items;
      }, { immediate: true, emptyNote: "No other snapshot has a workspace this one lacks." });
      slot.replaceChildren(p);
      p.focusInput();
    };
    const seg = h("div", { class: "seg" },
      h("button", { onclick: (e) => { mark(e); fromOpen(); } }, "Open in cmux"),
      h("button", { onclick: (e) => { mark(e); fromSnapshot(); } }, "From another snapshot"));
    function mark(e) {
      for (const b of seg.children) b.classList.toggle("active", b === e.currentTarget);
    }
    return h("div", { class: "add-ws-panel" },
      h("div", { class: "row" }, h("strong", { class: "grow" }, "Add workspace"),
        iconButton("close", "Cancel", () => wrap.replaceChildren(closed()))),
      h("div", { class: "row", id: "add-ws-from" }, seg),
      slot);
  };
  wrap.append(closed());
  return wrap;
}

function saveBar(name) {
  const ops = opsOf(name);
  if (ops.length === 0) return null;
  const message = h("input", { class: "grow", placeholder: "Describe the change (optional)", "aria-label": "Version message" });
  const save = actionButton("Save version", async () => {
    const args = ["edit", name, "--batch", "-"];
    if (message.value.trim()) args.push("-m", message.value.trim());
    const r = await ccsnap(args, JSON.stringify(ops));
    pending.delete(name);
    toast(r.exit === 3 ? "No changes to save." : `Saved v${r.result.version}: ${r.result.message}`);
    render();
  }, { class: "primary" });
  message.addEventListener("keydown", (e) => { if (e.key === "Enter") save.click(); });
  const discard = h("button", { class: "ghost", onclick: () => {
    pending.delete(name);
    render();
  } }, "Discard");
  const undo = h("button", { class: "ghost", title: "Undo the last change", onclick: () => {
    const group = opGroups.get(ops[ops.length - 1]) || [];
    ops.splice(ops.length - Math.max(group.length, 1));
    render();
  } }, "Undo");
  // Ops added together (a drag between workspaces is two) share one line.
  const lines = [...new Set(ops.map((op) => opLabels.get(op) || op.op))];
  const list = h("ul", { class: "changes-list small muted hidden" }, lines.map((l) => h("li", {}, l)));
  const toggle = h("button", { class: "changes-toggle", "aria-expanded": "false", title: "Show the changes", onclick: () => {
    const open = list.classList.toggle("hidden") === false;
    toggle.setAttribute("aria-expanded", String(open));
    toggle.replaceChildren(icon(open ? "chevdown" : "chevron"), plural(ops.length, "unsaved change"));
  } }, icon("chevron"), plural(ops.length, "unsaved change"));
  return h("div", { class: "savebar" },
    h("div", { class: "row" }, h("div", { class: "grow" }, toggle), undo, discard, save),
    list,
    h("div", { class: "row" }, message));
}

// ---- History ----

function describeChange(c) {
  switch (c.change) {
    case "workspace-added": return ["add", `Workspace ${c.workspace}`];
    case "workspace-removed": return ["del", `Workspace ${c.workspace}`];
    case "tab-added": return ["add", `${c.title}`, c.workspace];
    case "tab-removed": return ["del", `${c.title}`, c.workspace];
    case "tab-moved": return ["mod", `${c.title}`, `${c.from} → ${c.to}`];
    case "tab-renamed": return ["mod", `${c.from} → ${c.to}`, c.workspace];
    default: return ["mod", JSON.stringify(c)];
  }
}
const SIGNS = { add: "+", del: "−", mod: "~" };

async function renderHistory(name) {
  const { result } = await ccsnap(["history", name]);
  const versions = [...result.versions].reverse();
  const picked = []; // up to two version numbers, in the order ticked
  const out = h("div", {});
  const label = h("span", { class: "muted small grow" });
  const compare = actionButton("Compare", async () => {
    const [a, b] = [...picked].sort((x, y) => x - y);
    const d = await ccsnap(["diff", `${name}@${a}`, `${name}@${b}`]);
    out.replaceChildren(h("div", { class: "diff" },
      h("h2", {}, `v${a} → v${b}`, h("span", { class: "muted small" }, `  ${d.result.summary}`)),
      d.result.changes.length === 0
        ? h("p", { class: "muted" }, "No differences.")
        : h("ul", {}, d.result.changes.map((c) => {
          const [kind, text, sub] = describeChange(c);
          return h("li", { class: kind }, h("span", { class: "sign" }, SIGNS[kind]),
            h("div", { class: "grow" }, h("div", {}, text), sub ? h("div", { class: "muted small" }, sub) : null));
        }))));
  });
  const rows = new Map();
  const sync = () => {
    compare.disabled = picked.length !== 2;
    const sorted = [...picked].sort((x, y) => x - y);
    label.textContent = picked.length === 2 ? `v${sorted[0]} → v${sorted[1]}` : picked.length === 1 ? "Pick one more version" : "Pick two versions to compare";
    for (const [v, { li, box }] of rows) {
      box.checked = picked.includes(v);
      li.classList.toggle("picked", box.checked);
    }
  };
  // Pre-pick the two newest, the comparison people want most.
  if (versions.length >= 2) picked.push(versions[1].version, versions[0].version);
  const list = h("ul", { class: "list versions" }, versions.map((v, i) => {
    const box = h("input", { type: "checkbox", "aria-label": `Compare v${v.version}` });
    box.addEventListener("change", () => {
      const at = picked.indexOf(v.version);
      if (box.checked && at < 0) {
        picked.push(v.version);
        if (picked.length > 2) picked.shift();
      } else if (!box.checked && at >= 0) {
        picked.splice(at, 1);
      }
      sync();
    });
    const li = h("li", { class: "item" },
      h("label", { class: "row grow" }, box,
        h("div", { class: "grow" },
          h("div", { class: "meta" }, h("strong", {}, `v${v.version}`), i === 0 ? h("span", { class: "pill accent" }, "latest") : null,
            h("span", { class: "grow" }), h("span", { class: "muted small" }, time(v.createdAt))),
          h("div", {}, v.message || h("span", { class: "faint" }, "No message")),
          h("div", { class: "muted small" }, `${plural(v.workspaces, "workspace")} · ${plural(v.tabs, "tab")}`))),
      h("button", { class: "ghost small", title: `Restore tabs from v${v.version}`,
        onclick: () => go("snapshot", { tab: "restore", version: i === 0 ? null : v.version }) }, "Restore…"));
    rows.set(v.version, { li, box });
    return li;
  }));
  sync();
  return h("div", { class: "stack" },
    versions.length > 1 ? h("div", { class: "row" }, label, compare) : null,
    list,
    out);
}

// ---- Restore ----

async function renderRestore(name) {
  const ref = state.version ? `${name}@${state.version}` : name;
  const { result } = await ccsnap(["show", ref]);
  const boxes = []; // { ws, pos, box }
  const out = h("div", {});
  const count = h("span", { class: "muted small grow" });
  const all = h("input", { type: "checkbox", checked: true, "aria-label": "Select all" });
  const sync = () => {
    const n = boxes.filter((b) => b.box.checked).length;
    count.textContent = `${n} of ${plural(boxes.length, "tab")} selected`;
    all.checked = n === boxes.length;
    all.indeterminate = n > 0 && n < boxes.length;
    preview.disabled = n === 0;
    out.replaceChildren(); // an earlier plan no longer matches the selection
  };
  const wsBoxes = [];
  const tree = result.version.workspaces.map((w) => {
    const key = `restore/${ref}/${w.name}`;
    const wsBox = h("input", { type: "checkbox", checked: true, "aria-label": `All tabs of ${w.name}` });
    const tabBoxes = w.tabs.map((t, i) => {
      const box = h("input", { type: "checkbox", checked: true });
      boxes.push({ ws: w.name, pos: i + 1, box });
      box.addEventListener("change", () => {
        syncWs();
        sync();
      });
      return box;
    });
    const syncWs = () => {
      wsBox.checked = tabBoxes.every((b) => b.checked);
      wsBox.indeterminate = !wsBox.checked && tabBoxes.some((b) => b.checked);
    };
    wsBoxes.push(syncWs);
    wsBox.addEventListener("change", () => {
      for (const b of tabBoxes) b.checked = wsBox.checked;
      wsBox.indeterminate = false;
      sync();
    });
    const bodyEl = h("ol", { class: expanded.has(key) ? "ws-body" : "ws-body hidden" }, w.tabs.map((t, i) =>
      h("li", { class: "tab" }, h("label", { class: "grow" }, tabBoxes[i], kindIcon(t), h("span", { class: "ellipsis" }, t.title)))));
    const toggle = h("button", { class: `icon toggle ${expanded.has(key) ? "open" : ""}`, title: "Show tabs", "aria-label": "Show tabs",
      onclick: () => {
        const open = !expanded.has(key);
        open ? expanded.add(key) : expanded.delete(key);
        toggle.classList.toggle("open", open);
        bodyEl.classList.toggle("hidden", !open);
      } }, icon("chevron"));
    return h("div", { class: "ws" },
      h("div", { class: "ws-head" }, toggle,
        h("label", { class: "grow" }, wsBox, h("span", { class: "name ellipsis" }, w.name)),
        h("span", { class: "pill" }, w.tabs.length)),
      bodyEl);
  });
  all.addEventListener("change", () => {
    for (const b of boxes) b.box.checked = all.checked;
    wsBoxes.forEach((f) => f());
    sync();
  });
  const preview = actionButton("Preview restore", async () => {
    const chosen = boxes.filter((b) => b.box.checked);
    if (chosen.length === 0) return toast("Pick at least one tab.", "error");
    const args = ["restore", ref, "--plan"];
    if (chosen.length < boxes.length) {
      for (const b of chosen) args.push("--tab", `${b.ws}#${b.pos}`);
    }
    out.replaceChildren(renderPlan(await ccsnap(args)));
    out.scrollIntoView({ behavior: "smooth", block: "nearest" });
  }, { class: "primary" });
  sync();
  return h("div", { class: "stack" },
    state.version
      ? h("div", { class: "banner" }, h("span", { class: "grow" }, `Restoring from v${state.version}, an older version.`),
        h("button", { class: "link", onclick: () => go("snapshot", { tab: "restore", version: null }) }, "Use latest"))
      : null,
    h("p", { class: "hint" }, "Only tabs that are not already open in cmux will open. Nothing is closed."),
    h("div", { class: "row tree" }, h("label", {}, all, count)),
    h("div", { class: "tree" }, tree),
    h("div", { class: "footer-actions" }, preview),
    out);
}

function renderPlan(r) {
  const plan = r.result.plan;
  if (r.exit === 3 || plan.actions.length === 0) {
    return h("div", { class: "banner ok" }, icon("check"), "Nothing to restore: everything is already open.");
  }
  const tabs = plan.actions.reduce((n, a) => n + a.tabs.length, 0);
  const apply = actionButton(`Open ${plural(tabs, "tab")}`, async () => {
    const a = await ccsnap(["apply", plan.planId]);
    apply.replaceWith(renderSteps(a.result));
  }, { class: "primary" });
  return h("div", { class: "plan" },
    h("h2", {}, "This will open"),
    h("div", {}, plan.actions.map((a) => h("div", { class: "group" },
      h("div", { class: "row" }, icon(a.action === "create-workspace" ? "plus" : "chevron"),
        a.action === "create-workspace" ? h("span", {}, "New workspace ", h("strong", {}, a.name)) : h("span", {}, "Add to ", h("strong", {}, a.workspaceName))),
      h("ul", {}, a.tabs.map((t) => h("li", { class: "ellipsis" }, t.title)))))),
    plan.skipped.length
      ? h("details", {}, h("summary", {}, `${plan.skipped.length} already open or skipped`),
        h("ul", { class: "muted small" }, plan.skipped.map((s) => h("li", {}, s))))
      : null,
    plan.warnings.length ? h("ul", { class: "banner warn warnings" }, plan.warnings.map((w) => h("li", {}, w))) : null,
    h("div", { class: "row" }, h("span", { class: "grow" }), apply));
}

function renderSteps(result) {
  return h("div", { class: "stack" },
    h("div", { class: `banner ${result.failed ? "warn" : "ok"}` }, icon(result.failed ? "warn" : "check"),
      result.failed ? `${plural(result.failed, "step")} failed.` : "Restored."),
    h("ul", { class: "steps small" }, result.steps.map((s) =>
      h("li", { class: s.ok ? "" : "error" }, icon(s.ok ? "check" : "close"), h("span", {}, s.description, s.error ? `: ${s.error}` : "")))));
}

// ---- Recent ----

async function viewRecent() {
  const hours = state.hours;
  const r = await ccsnap(["recent", "--hours", String(hours)]);
  const out = h("div", {});
  const pick = h("select", { "aria-label": "Period", onchange: (e) => go("recent", { hours: Number(e.target.value) }) },
    [24, 72, 168].map((n) => h("option", { value: n, selected: n === hours }, n === 24 ? "Last 24 hours" : `Last ${n / 24} days`)));
  const sessions = r.result.sessions;
  return h("section", { class: "stack" },
    h("div", { class: "row" }, h("div", { class: "grow" }, h("h1", {}, "Not open in cmux"),
      h("p", { class: "muted small" }, "Claude sessions you used recently that have no tab now.")), pick),
    sessions.length === 0
      ? h("div", { class: "empty" }, h("strong", {}, "Nothing missing"), "Every recent Claude session has a tab in cmux.")
      : h("div", { class: "stack" },
        h("ul", { class: "list" }, sessions.map((s) => h("li", { class: "item" },
          h("span", { class: "kind claude" }, icon("claude")),
          h("div", { class: "grow" },
            h("div", { class: "meta" }, h("span", { class: "ellipsis grow" }, s.title || s.firstPrompt || s.sessionId),
              h("span", { class: "muted small" }, time(s.modified))),
            h("div", { class: "mono muted ellipsis", title: s.cwd }, tilde(s.cwd)))))),
        h("div", { class: "footer-actions" }, actionButton(`Preview restoring ${plural(sessions.length, "session")}`, async () => {
          out.replaceChildren(renderPlan(await ccsnap(["recent", "--hours", String(hours), "--plan"])));
          out.scrollIntoView({ behavior: "smooth", block: "nearest" });
        }, { class: "primary" })),
        out));
}

for (const b of document.querySelectorAll("nav button")) b.addEventListener("click", () => go(b.dataset.view));
const refresh = document.getElementById("refresh");
refresh.append(icon("refresh"));
refresh.addEventListener("click", render);
// Coming back to the page shows fresh data, unless the user is in the middle of typing.
document.addEventListener("visibilitychange", () => {
  // Restore keeps the user's ticks, so it is left alone.
  const restoring = state.view === "snapshot" && state.tab === "restore";
  if (document.visibilityState === "visible" && !restoring && !document.activeElement?.matches("input, select")) render();
});
// `ccsnap ui` and Reconnect point an open tab at the key by changing only the fragment.
window.addEventListener("hashchange", () => {
  TOKEN = takeToken();
  render();
});
render();
