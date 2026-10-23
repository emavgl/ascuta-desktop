// Settings UI. Talks to the Rust core over the loopback JSON API; the
// per-run token comes from the URL the app opened (?t=...).
const token = new URLSearchParams(location.search).get("t") || sessionStorage.getItem("t") || "";
if (token) sessionStorage.setItem("t", token);

const LANGS = ["auto", "en", "it", "de", "fr", "es", "pt", "nl", "pl", "ru", "ja", "zh"];
let state = null;
let busy = false;

async function api(path, body) {
  const r = await fetch(path, {
    method: body === undefined ? "GET" : "POST",
    headers: { "X-Ascuta-Token": token },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const j = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(j.error || r.statusText);
  return j;
}

function toast(msg) {
  const t = document.getElementById("toast");
  t.textContent = msg;
  t.classList.add("show");
  setTimeout(() => t.classList.remove("show"), 2200);
}

const el = (tag, props = {}, ...kids) => {
  const n = Object.assign(document.createElement(tag), props);
  for (const k of kids.flat()) if (k != null) n.append(k);
  return n;
};

function row(title, subtitle, control, opts = {}) {
  return el("div", { className: "row" },
    el("div", { className: "text" },
      el("div", { className: "title" }, title),
      subtitle ? el("div", { className: "subtitle" + (opts.err ? " err" : "") }, subtitle) : null,
      opts.extra),
    control);
}

function toggle(checked, onChange) {
  const input = el("input", { type: "checkbox", checked });
  input.addEventListener("change", () => onChange(input.checked));
  return el("label", { className: "switch" }, input, el("span"));
}

async function save(patch, msg = "Saved") {
  try {
    await api("/api/settings", patch);
    toast(msg);
  } catch (e) {
    toast(e.message);
  }
  await refresh();
}

function mb(n) { return Math.round(n / 1048576); }

function modelRow(m, current) {
  const st = m.status;
  const inUse = m.id === current;
  let sub = m.description, control = null, extra = null, err = false;
  if (st.state === "downloading") {
    const pct = st.total ? Math.floor((st.done * 100) / st.total) : null;
    sub = pct == null ? `Downloading… ${mb(st.done)} MB` : `Downloading ${pct}%… (${mb(st.done)} / ${mb(st.total)} MB)`;
    extra = el("progress", { max: 100, value: pct ?? undefined });
  } else if (st.state === "failed") {
    sub = `Download failed: ${st.error}`;
    err = true;
    control = el("button", { onclick: () => download(m.id) }, "Retry");
  } else if (st.state === "ready") {
    sub = `${m.description} — downloaded ✓`;
    control = el("span", {},
      inUse ? el("span", { className: "sub" }, "In use") :
        el("button", { className: "text", onclick: () => save({ model: m.id }, "Model changed") }, "Use"),
      el("button", { className: "text danger", onclick: () => remove(m.id) }, "✕ Delete"));
  } else {
    control = el("button", { onclick: () => download(m.id) }, "↓ Download");
  }
  return row(m.id, sub, control, { extra, err });
}

async function download(id) {
  try { await api("/api/download", { model: id }); } catch (e) { toast(e.message); }
  await refresh();
}

async function remove(id) {
  const inUse = state.settings.model === id;
  const msg = inUse
    ? `Delete ${id}? It is the model Ascuta is using; dictation stops working until you download one again.`
    : `Delete ${id}? This frees the disk space.`;
  if (!confirm(msg)) return;
  try { await api("/api/delete", { model: id }); toast("Deleted"); } catch (e) { toast(e.message); }
  await refresh();
}

function render() {
  const s = state.settings;
  const banner = document.getElementById("banner");
  banner.className = "banner" + (state.readiness.ready ? "" : " warn");
  banner.replaceChildren(el("b", {}, state.readiness.ready ? "All set" : "Setup needed"),
    state.readiness.ready ? "Hold the hotkey, speak, release: the text is typed where your cursor is."
      : state.readiness.message);

  const langSel = el("select", {}, LANGS.map((l) => el("option", { value: l, selected: l === s.language }, l)));
  langSel.addEventListener("change", () => save({ language: langSel.value }));

  const hot = el("input", { type: "text", value: s.hotkey, size: 12 });
  hot.addEventListener("change", () => save({ hotkey: hot.value }, "Saved — restart Ascuta to apply the hotkey"));

  const modeSel = el("select", {},
    el("option", { value: "type", selected: s.injection_mode === "type" }, "Direct input (typing)"),
    el("option", { value: "paste", selected: s.injection_mode === "paste" }, "Copy & paste (clipboard)"));
  modeSel.addEventListener("change", () => save({ injection_mode: modeSel.value }));

  const port = el("input", { type: "number", value: s.bridge_port, min: 1024, max: 65535 });
  port.addEventListener("change", () => save({ bridge_port: Number(port.value) }, "Saved — restart to apply"));

  const out = [
    el("h2", {}, "Current configuration"),
    row("Voice model", s.model, null),
    row("Language", "auto-detect or a fixed language", langSel),

    el("h2", {}, "Common"),
    row("Hotkey", "Hold to talk (applied at next start)", hot),
    row("How text is inserted", "Typing sends characters; copy & paste is faster and more compatible",
      modeSel),
    row("Restore clipboard", "Put back your clipboard after pasting",
      toggle(s.preserve_clipboard, (v) => save({ preserve_clipboard: v }))),
    row("Launch at login", "Start Ascuta when you sign in",
      toggle(s.launch_at_login, (v) => save({ launch_at_login: v }))),

    row("Restart Ascuta", "Applies hotkey, port and phone-bridge changes",
      el("button", { onclick: async () => { try { await api("/api/restart", {}); toast("Restarting…"); } catch (e) { toast(e.message); } } }, "Restart")),

    el("h2", {}, "Models"),
    el("p", { className: "note" }, "Stored in ", el("code", {}, state.models_dir)),
    ...state.models.map((m) => modelRow(m, s.model)),

    el("h2", {}, "Input from phone"),
    el("p", { className: "note" },
      "The Ascuta Android app can type its transcripts on this computer, over your local network."),
    row("Send from phone", "Receive text from a paired phone (applied at next start)",
      toggle(s.bridge_enabled, (v) => save({ bridge_enabled: v }, "Saved — restart to apply"))),
    row("Port", "Your firewall must allow it", port),
  ];
  if (state.pairing) {
    out.push(el("p", { className: "note" }, `Pair from the phone: Home → Send to computer → scan. (${state.pairing.name} · ${state.pairing.host}:${state.pairing.port})`));
    out.push(el("pre", { className: "qr" }, state.pairing.qr));
  } else if (s.bridge_enabled) {
    out.push(el("p", { className: "note" }, "Restart Ascuta to show the pairing code."));
  }

  out.push(el("h2", {}, "Files"));
  out.push(row("Settings file", state.settings_path, null));
  const app = document.getElementById("app");
  // Don't clobber a text field the user is typing in.
  if (!app.contains(document.activeElement) || document.activeElement.tagName === "BODY") {
    app.replaceChildren(...out);
  }
}

async function refresh() {
  try {
    state = await api("/api/state");
    render();
  } catch (e) {
    document.getElementById("banner").replaceChildren(el("b", {}, "Cannot reach Ascuta"),
      "Is the app running? Reopen this page from the tray menu.");
  }
}

refresh();
setInterval(() => {
  if (state && state.models.some((m) => m.status.state === "downloading")) refresh();
}, 1000);
