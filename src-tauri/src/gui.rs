//! Settings GUI: a local web page served by the Rust core.
//!
//! The Tauri webview needs webkit2gtk system libs and is fragile on Wayland
//! (PLAN §3/§8), so the settings window is served from `127.0.0.1` and opened
//! in the default browser. Same HTML/JS can later be hosted in Tauri.
//!
//! Security: bound to loopback only; every `/api/*` call must carry the
//! per-run random token (from the URL the app opens), which defeats
//! cross-site requests and DNS rebinding. Routing is a pure function
//! ([`route`]) so tests need no sockets.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use tiny_http::{Header, Response, Server};

use crate::downloader::{DlStatus, Tracker};
use crate::pairing::{self, Pairing};
use crate::{models, readiness, settings, OsEvent};

pub const DEFAULT_GUI_PORT: u16 = 8738;

const INDEX_HTML: &str = include_str!("../../src/index.html");
const APP_JS: &str = include_str!("../../src/app.js");
const STYLES_CSS: &str = include_str!("../../src/styles.css");

pub struct Ctx {
    pub token: String,
    pub store: settings::Store,
    pub models_dir: std::path::PathBuf,
    pub tracker: Tracker,
    /// Set when the phone bridge is running in this process.
    pub pairing: Option<Pairing>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub token: Option<String>,
    pub body: String,
}

pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
    pub event: Option<OsEvent>,
}

impl Reply {
    fn json(status: u16, v: Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: v.to_string(),
            event: None,
        }
    }
    fn err(status: u16, msg: &str) -> Self {
        Self::json(status, json!({"ok": false, "error": msg}))
    }
    fn asset(content_type: &'static str, body: &str) -> Self {
        Self {
            status: 200,
            content_type,
            body: body.to_string(),
            event: None,
        }
    }
}

/// Subsets of settings the UI may change. Port/model/etc. are validated.
fn apply_settings(cur: &mut settings::Settings, body: &Value) -> Result<(), String> {
    if let Some(m) = body.get("model").and_then(Value::as_str) {
        if !models::is_known_model(m) {
            return Err(format!("unknown model: {m}"));
        }
        cur.model = m.to_string();
    }
    if let Some(l) = body.get("language").and_then(Value::as_str) {
        let ok = l == "auto"
            || (l.len() == 2 && l.chars().all(|c| c.is_ascii_lowercase()));
        if !ok {
            return Err("language must be `auto` or a 2-letter code".into());
        }
        cur.language = l.to_string();
    }
    if let Some(h) = body.get("hotkey").and_then(Value::as_str) {
        let h = h.trim();
        if h.is_empty() || h.len() > 40 || !crate::chord::Chord::parse(h).is_ok_and(|c| c.is_safe()) {
            return Err(
                "invalid hotkey (a lone letter/digit would block typing; add Ctrl/Alt/Win)"
                    .into(),
            );
        }
        // Store the canonical spelling (`Ctrl+Win`, `RightAlt`, …).
        cur.hotkey = crate::chord::Chord::parse(h)
            .map(|c| c.to_string())
            .unwrap_or_else(|_| h.to_string());
    }
    if let Some(b) = body.get("preserve_clipboard").and_then(Value::as_bool) {
        cur.preserve_clipboard = b;
    }
    if let Some(m) = body.get("injection_mode").and_then(Value::as_str) {
        if m != "type" && m != "paste" {
            return Err("injection mode must be `type` or `paste`".into());
        }
        cur.injection_mode = m.to_string();
    }
    if let Some(b) = body.get("launch_at_login").and_then(Value::as_bool) {
        cur.launch_at_login = b;
    }
    if let Some(b) = body.get("cleanup_enabled").and_then(Value::as_bool) {
        cur.cleanup_enabled = b;
    }
    if let Some(b) = body.get("bridge_enabled").and_then(Value::as_bool) {
        cur.bridge_enabled = b;
    }
    if let Some(p) = body.get("bridge_port").and_then(Value::as_u64) {
        if !(1024..=65535).contains(&p) {
            return Err("bridge port must be 1024-65535".into());
        }
        cur.bridge_port = p as u16;
    }
    Ok(())
}

/// Keyboard access state for the global hotkey (Linux `input` group).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardAccess {
    Ok,
    /// Added to the group but this session predates it.
    NeedsRelogin,
    NeedsGroup,
}

pub fn keyboard_access_from(can_read: bool, in_group: bool) -> KeyboardAccess {
    if can_read {
        KeyboardAccess::Ok
    } else if in_group {
        KeyboardAccess::NeedsRelogin
    } else {
        KeyboardAccess::NeedsGroup
    }
}

#[cfg(target_os = "linux")]
fn keyboard_access() -> KeyboardAccess {
    let can_read = std::fs::read_dir("/dev/input")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("event"))
        .any(|e| std::fs::File::open(e.path()).is_ok());
    let user = std::env::var("USER").unwrap_or_default();
    let in_group = std::fs::read_to_string("/etc/group")
        .unwrap_or_default()
        .lines()
        .any(|l| {
            let mut f = l.split(':');
            f.next() == Some("input")
                && f.nth(2).is_some_and(|m| m.split(',').any(|u| u == user))
        });
    keyboard_access_from(can_read, in_group)
}

#[cfg(not(target_os = "linux"))]
fn keyboard_access() -> KeyboardAccess {
    KeyboardAccess::Ok
}

/// Ask polkit (graphical password prompt) to add the user to `input`.
fn enable_keyboard_access() -> Result<(), String> {
    let user = std::env::var("USER").map_err(|_| "unknown user".to_string())?;
    let status = std::process::Command::new("pkexec")
        .args(["usermod", "-aG", "input", &user])
        .status()
        .map_err(|e| format!("pkexec not available: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("permission request was cancelled or failed".into())
    }
}

fn state_json(ctx: &Ctx) -> Value {
    let s = ctx.store.read().unwrap_or_default();
    let describe = |m: &models::ModelSpec| {
        let ready = models::missing_files(&ctx.models_dir, m.id).is_empty();
        let status = match ctx.tracker.status(m.id) {
            Some(DlStatus::Downloading { done, total }) => {
                json!({"state": "downloading", "done": done, "total": total})
            }
            Some(DlStatus::Failed(e)) => json!({"state": "failed", "error": e}),
            None if ready => json!({"state": "ready"}),
            None => json!({"state": "missing"}),
        };
        json!({
            "id": m.id,
            "description": m.description,
            "family": format!("{:?}", m.family).to_lowercase(),
            "status": status,
        })
    };
    let model_list: Vec<Value> = models::ALL_MODELS.iter().map(describe).collect();
    let cleanup_model = models::find_model(models::DEFAULT_CLEANUP_MODEL)
        .map(|m| describe(&m))
        .unwrap_or(Value::Null);
    let r = readiness::dictation_readiness(&ctx.models_dir, &s.model);
    let pairing = ctx.pairing.as_ref().map(|p| {
        json!({
            "uri": p.to_uri(),
            "name": p.name,
            "host": p.host,
            "port": p.port,
            "qr": p.render_unicode().unwrap_or_default(),
        })
    });
    json!({
        "settings": {
            "model": s.model,
            "language": s.language,
            "hotkey": s.hotkey,
            "preserve_clipboard": s.preserve_clipboard,
            "injection_mode": s.injection_mode,
            "launch_at_login": s.launch_at_login,
            "cleanup_enabled": s.cleanup_enabled,
            "bridge_enabled": s.bridge_enabled,
            "bridge_port": s.bridge_port,
        },
        "models": model_list,
        "cleanup_model": cleanup_model,
        "cleanup_available": true,
        "cleanup_runtime_ready": crate::cleanup::runtime_ready(),
        "readiness": {"ready": r.code == readiness::ReadinessCode::Ready, "message": r.message},
        "models_dir": ctx.models_dir.display().to_string(),
        "settings_path": settings::Store::path().display().to_string(),
        "pairing": pairing,
        "bridge_running": ctx.pairing.is_some(),
        "keyboard": match keyboard_access() {
            KeyboardAccess::Ok => "ok",
            KeyboardAccess::NeedsRelogin => "needs_relogin",
            KeyboardAccess::NeedsGroup => "needs_group",
        },
    })
}

/// Pure router. `on_download_done` builds the callback run after a model
/// download finishes (so tests can avoid threads).
pub fn route(
    ctx: &Ctx,
    req: &Request,
    on_download_done: &dyn Fn() -> Box<dyn FnOnce() + Send>,
) -> Reply {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") | ("GET", "/index.html") => {
            return Reply::asset("text/html; charset=utf-8", INDEX_HTML)
        }
        ("GET", "/app.js") => {
            return Reply::asset("application/javascript; charset=utf-8", APP_JS)
        }
        ("GET", "/styles.css") => return Reply::asset("text/css; charset=utf-8", STYLES_CSS),
        _ => {}
    }
    if !req.path.starts_with("/api/") {
        return Reply::err(404, "not found");
    }
    let presented = req.token.as_deref().unwrap_or("");
    if !pairing::tokens_match(&ctx.token, presented) {
        return Reply::err(401, "unauthorized");
    }
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/state") => Reply::json(200, state_json(ctx)),
        ("POST", "/api/settings") => {
            let Ok(body) = serde_json::from_str::<Value>(&req.body) else {
                return Reply::err(400, "bad json");
            };
            let mut cur = ctx.store.read().unwrap_or_default();
            if let Err(e) = apply_settings(&mut cur, &body) {
                return Reply::err(400, &e);
            }
            if let Err(e) = ctx.store.save(&cur) {
                return Reply::err(500, &format!("cannot save: {e:#}"));
            }
            let mut r = Reply::json(200, json!({"ok": true}));
            r.event = Some(OsEvent::SettingsChanged);
            r
        }
        ("POST", "/api/download") => {
            let model = serde_json::from_str::<Value>(&req.body)
                .ok()
                .and_then(|v| v.get("model").and_then(Value::as_str).map(String::from));
            let Some(model) = model.filter(|m| models::find_model(m).is_some()) else {
                return Reply::err(400, "unknown model");
            };
            let started =
                ctx.tracker
                    .start(ctx.models_dir.clone(), &model, on_download_done());
            Reply::json(200, json!({"ok": true, "started": started}))
        }
        ("POST", "/api/restart") => {
            let mut r = Reply::json(200, json!({"ok": true}));
            r.event = Some(OsEvent::RestartRequested);
            r
        }
        ("POST", "/api/enable_keyboard") => match enable_keyboard_access() {
            Ok(()) => Reply::json(200, json!({"ok": true})),
            Err(e) => Reply::err(500, &e),
        },
        ("POST", "/api/delete") => {
            let model = serde_json::from_str::<Value>(&req.body)
                .ok()
                .and_then(|v| v.get("model").and_then(Value::as_str).map(String::from));
            let Some(spec) = model.as_deref().and_then(models::find_model) else {
                return Reply::err(400, "unknown model");
            };
            if matches!(
                ctx.tracker.status(spec.id),
                Some(DlStatus::Downloading { .. })
            ) {
                return Reply::err(409, "download in progress");
            }
            // `extracted_dir` comes from the static catalog, never the request.
            let dir = ctx.models_dir.join(spec.extracted_dir);
            if dir.is_dir() {
                if let Err(e) = std::fs::remove_dir_all(&dir) {
                    return Reply::err(500, &format!("cannot delete: {e}"));
                }
            }
            let mut r = Reply::json(200, json!({"ok": true}));
            r.event = Some(OsEvent::SettingsChanged);
            r
        }
        _ => Reply::err(404, "not found"),
    }
}

pub struct Gui {
    addr: SocketAddr,
    token: String,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Gui {
    /// Serve on `127.0.0.1:port`, falling back to an ephemeral port.
    pub fn start(ctx: Ctx, port: u16, tx: Sender<OsEvent>) -> Result<Self> {
        let server = Server::http(format!("127.0.0.1:{port}"))
            .or_else(|_| Server::http("127.0.0.1:0"))
            .map_err(|e| anyhow!("cannot bind settings GUI: {e}"))?;
        let addr = server
            .server_addr()
            .to_ip()
            .ok_or_else(|| anyhow!("no ip address"))?;
        let token = ctx.token.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || serve(server, ctx, tx, thread_stop));
        Ok(Self {
            addr,
            token,
            stop,
            handle: Some(handle),
        })
    }

    /// `http://127.0.0.1:PORT` — for the native settings window.
    pub fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// URL to open in the browser (carries the session token).
    pub fn url(&self) -> String {
        format!("http://{}/?t={}", self.addr, self.token)
    }
}

impl Drop for Gui {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn serve(server: Server, ctx: Ctx, tx: Sender<OsEvent>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        let Ok(Some(mut request)) = server.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        let method = request.method().as_str().to_string();
        let path = request.url().split('?').next().unwrap_or("").to_string();
        let token = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("X-Ascuta-Token"))
            .map(|h| h.value.as_str().to_string());
        let mut body = String::new();
        {
            use std::io::Read;
            let _ = request
                .as_reader()
                .take(64 * 1024)
                .read_to_string(&mut body);
        }
        let req = Request {
            method,
            path,
            token,
            body,
        };
        let dl_tx = tx.clone();
        let reply = route(&ctx, &req, &move || {
            let tx = dl_tx.clone();
            Box::new(move || {
                let _ = tx.send(OsEvent::ModelReady);
            })
        });
        if let Some(ev) = reply.event {
            let _ = tx.send(ev);
        }
        let mut resp = Response::from_string(reply.body).with_status_code(reply.status);
        if let Ok(h) = Header::from_bytes("Content-Type", reply.content_type) {
            resp = resp.with_header(h);
        }
        if let Ok(h) = Header::from_bytes("Cache-Control", "no-store") {
            resp = resp.with_header(h);
        }
        let _ = request.respond(resp);
    }
}

/// Open `url` in the default browser; failure only prints the URL.
pub fn open_browser(url: &str) {
    let result = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    if result.is_err() {
        eprintln!("ascuta: open this page in your browser: {url}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(dir: &std::path::Path) -> Ctx {
        Ctx {
            token: "tok".into(),
            store: settings::Store::with_path(dir.join("settings.json")),
            models_dir: dir.join("models"),
            tracker: Tracker::default(),
            pairing: None,
        }
    }

    fn req(method: &str, path: &str, token: Option<&str>, body: &str) -> Request {
        Request {
            method: method.into(),
            path: path.into(),
            token: token.map(String::from),
            body: body.into(),
        }
    }

    fn noop() -> Box<dyn FnOnce() + Send> {
        Box::new(|| {})
    }

    #[test]
    fn serves_assets_without_token() {
        let d = tempfile::tempdir().unwrap();
        let c = ctx(d.path());
        let r = route(&c, &req("GET", "/", None, ""), &noop);
        assert_eq!(r.status, 200);
        assert!(r.body.contains("ascuta"));
    }

    #[test]
    fn api_requires_token() {
        let d = tempfile::tempdir().unwrap();
        let c = ctx(d.path());
        assert_eq!(route(&c, &req("GET", "/api/state", None, ""), &noop).status, 401);
        assert_eq!(
            route(&c, &req("GET", "/api/state", Some("bad"), ""), &noop).status,
            401
        );
        assert_eq!(
            route(&c, &req("GET", "/api/state", Some("tok"), ""), &noop).status,
            200
        );
    }

    #[test]
    fn state_lists_models_and_missing() {
        let d = tempfile::tempdir().unwrap();
        let c = ctx(d.path());
        let r = route(&c, &req("GET", "/api/state", Some("tok"), ""), &noop);
        let v: Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["models"].as_array().unwrap().len(), models::ALL_MODELS.len());
        assert_eq!(v["models"][0]["status"]["state"], "missing");
        assert_eq!(v["readiness"]["ready"], false);
    }

    #[test]
    fn saves_valid_settings_and_notifies() {
        let d = tempfile::tempdir().unwrap();
        let c = ctx(d.path());
        let body = r#"{"model":"whisper-base-int8","language":"it","bridge_port":9000}"#;
        let r = route(&c, &req("POST", "/api/settings", Some("tok"), body), &noop);
        assert_eq!(r.status, 200);
        assert_eq!(r.event, Some(OsEvent::SettingsChanged));
        let s = c.store.read().unwrap();
        assert_eq!(s.model, "whisper-base-int8");
        assert_eq!(s.language, "it");
        assert_eq!(s.bridge_port, 9000);
    }

    #[test]
    fn rejects_invalid_settings_and_keeps_token() {
        let d = tempfile::tempdir().unwrap();
        let c = ctx(d.path());
        let mut s = settings::Settings::default();
        s.bridge_token = "keepme".into();
        c.store.save(&s).unwrap();
        for bad in [
            r#"{"model":"nope"}"#,
            r#"{"language":"../etc"}"#,
            r#"{"bridge_port":80}"#,
            r#"{"hotkey":"  "}"#,
            r#"{"hotkey":"Ctrl+Nope"}"#,
            r#"{"hotkey":"W"}"#,
            "not json",
        ] {
            let r = route(&c, &req("POST", "/api/settings", Some("tok"), bad), &noop);
            assert_eq!(r.status, 400, "{bad}");
        }
        // The UI cannot read or overwrite the pairing token.
        route(
            &c,
            &req("POST", "/api/settings", Some("tok"), r#"{"bridge_token":"x"}"#),
            &noop,
        );
        assert_eq!(c.store.read().unwrap().bridge_token, "keepme");
        let r = route(&c, &req("GET", "/api/state", Some("tok"), ""), &noop);
        assert!(!r.body.contains("keepme"));
    }

    #[test]
    fn keyboard_access_states() {
        assert_eq!(keyboard_access_from(true, false), KeyboardAccess::Ok);
        assert_eq!(keyboard_access_from(false, true), KeyboardAccess::NeedsRelogin);
        assert_eq!(keyboard_access_from(false, false), KeyboardAccess::NeedsGroup);
    }

    #[test]
    fn delete_removes_model_dir() {
        let d = tempfile::tempdir().unwrap();
        let c = ctx(d.path());
        let spec = models::PARAKEET_V2_INT8;
        let dir = c.models_dir.join(spec.extracted_dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("tokens.txt"), b"x").unwrap();
        let body = format!(r#"{{"model":"{}"}}"#, spec.id);
        let r = route(&c, &req("POST", "/api/delete", Some("tok"), &body), &noop);
        assert_eq!(r.status, 200);
        assert!(!dir.exists());
        let bad = route(
            &c,
            &req("POST", "/api/delete", Some("tok"), r#"{"model":"../.."}"#),
            &noop,
        );
        assert_eq!(bad.status, 400);
    }

    #[test]
    fn download_rejects_unknown_model() {
        let d = tempfile::tempdir().unwrap();
        let c = ctx(d.path());
        let r = route(
            &c,
            &req("POST", "/api/download", Some("tok"), r#"{"model":"x"}"#),
            &noop,
        );
        assert_eq!(r.status, 400);
    }

    #[test]
    fn loopback_end_to_end() {
        let d = tempfile::tempdir().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let gui = Gui::start(ctx(d.path()), 0, tx).unwrap();
        let url = gui.url();
        let base = url.split("/?").next().unwrap().to_string();
        let page = ureq::get(&format!("{base}/")).call().unwrap().into_string().unwrap();
        assert!(page.contains("<html"));
        let state: Value = serde_json::from_str(
            &ureq::get(&format!("{base}/api/state"))
                .set("X-Ascuta-Token", "tok")
                .call()
                .unwrap()
                .into_string()
                .unwrap(),
        )
        .unwrap();
        assert!(state["models"].is_array());
        ureq::post(&format!("{base}/api/settings"))
            .set("X-Ascuta-Token", "tok")
            .send_string(r#"{"language":"de"}"#)
            .unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), OsEvent::SettingsChanged);
    }
}
