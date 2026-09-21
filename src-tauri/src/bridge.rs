//! Phone bridge: a tiny blocking HTTP server that receives transcripts from
//! the Ascuta Android app and feeds them into the same injector as local
//! dictation.
//!
//! Design notes:
//! * No async runtime — `tiny_http` on its own thread matches the current
//!   non-tokio core (PLAN §3/§4) and keeps the headless CI gate light.
//! * The socket-thread does only HTTP; the **routing/auth logic is a pure
//!   function** ([`route`]) so `--check` and unit tests can exercise it with
//!   no sockets and no display server.
//! * A request that carries a valid token becomes an
//!   [`OsEvent::RemoteText`], which the winit loop (or the headless
//!   `--pair` loop) injects exactly like a locally transcribed result.
//! * Auth is a bearer token from the QR pairing payload; comparison is
//!   constant-time and an empty expected token denies everyone.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::json;
use tiny_http::{Header, Response, Server};

use crate::pairing::{self, Pairing};
use crate::OsEvent;

/// A decoded request, independent of `tiny_http` so it is easy to test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub authorization: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

impl HttpResponse {
    fn json(status: u16, value: serde_json::Value) -> Self {
        Self {
            status,
            body: value.to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct TextPayload {
    text: String,
    #[serde(default)]
    enter: bool,
}

/// Pure router. Returns the HTTP response plus, when the caller should act,
/// the event to inject into the core loop.
pub fn route(request: &HttpRequest, token: &str, name: &str) -> (HttpResponse, Option<OsEvent>) {
    let presented = request
        .authorization
        .as_deref()
        .and_then(bearer)
        .unwrap_or("");
    if !pairing::tokens_match(token, presented) {
        return (
            HttpResponse::json(401, json!({"ok": false, "error": "unauthorized"})),
            None,
        );
    }

    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/v1/ping") => (
            HttpResponse::json(200, json!({"ok": true, "name": name})),
            None,
        ),
        ("POST", "/v1/text") => match serde_json::from_str::<TextPayload>(&request.body) {
            Ok(payload) if !payload.text.is_empty() => (
                HttpResponse::json(200, json!({"ok": true})),
                Some(OsEvent::RemoteText {
                    text: payload.text,
                    enter: payload.enter,
                }),
            ),
            Ok(_) => (
                HttpResponse::json(400, json!({"ok": false, "error": "empty text"})),
                None,
            ),
            Err(e) => (
                HttpResponse::json(400, json!({"ok": false, "error": format!("bad json: {e}")})),
                None,
            ),
        },
        _ => (
            HttpResponse::json(404, json!({"ok": false, "error": "not found"})),
            None,
        ),
    }
}

fn bearer(value: &str) -> Option<&str> {
    let (scheme, token) = value.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("Bearer") {
        Some(token.trim())
    } else {
        None
    }
}

/// A running bridge. Dropping it stops the accept thread within one poll
/// interval (no async/`unblock` needed).
pub struct Bridge {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Bridge {
    /// Bind `bind:port` (use `0.0.0.0` to expose on the LAN, port `0` for an
    /// ephemeral test port) and start serving.
    pub fn start(
        bind: &str,
        port: u16,
        token: String,
        name: String,
        tx: Sender<OsEvent>,
    ) -> Result<Self> {
        let endpoint = format!("{bind}:{port}");
        let server = Server::http(&endpoint)
            .map_err(|e| anyhow!("cannot bind bridge on {endpoint}: {e}"))?;
        let addr = server
            .server_addr()
            .to_ip()
            .unwrap_or_else(|| SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || serve(server, token, name, tx, thread_stop));
        Ok(Self {
            addr,
            stop,
            handle: Some(handle),
        })
    }

    /// Address the server actually bound to.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Pairing payload for the QR, using the LAN IP when bound to `0.0.0.0`.
    pub fn pairing(&self, token: &str, name: &str) -> Pairing {
        let host = if self.addr.ip().is_unspecified() {
            lan_ip().unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
        } else {
            self.addr.ip()
        };
        Pairing {
            host: host.to_string(),
            port: self.addr.port(),
            token: token.to_string(),
            name: name.to_string(),
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn serve(server: Server, token: String, name: String, tx: Sender<OsEvent>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        match server.recv_timeout(Duration::from_millis(200)) {
            Ok(Some(mut request)) => {
                let decoded = read_request(&mut request);
                let (response, action) = route(&decoded, &token, &name);
                if let Some(event) = action {
                    if tx.send(event).is_err() {
                        break;
                    }
                }
                let header =
                    Header::from_bytes("Content-Type", "application/json").expect("static header");
                let http = Response::from_string(response.body)
                    .with_status_code(response.status)
                    .with_header(header);
                let _ = request.respond(http);
            }
            Ok(None) => {}
            Err(e) => eprintln!("ascuta: bridge recv error: {e}"),
        }
    }
}

fn read_request(request: &mut tiny_http::Request) -> HttpRequest {
    let method = request.method().as_str().to_string();
    let path = request.url().split('?').next().unwrap_or("").to_string();
    let authorization = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .map(|h| h.value.as_str().to_string());
    let mut body = String::new();
    let _ = request.as_reader().read_to_string(&mut body);
    HttpRequest {
        method,
        path,
        authorization,
        body,
    }
}

/// Best-effort LAN IP: connect a UDP socket to a public address and read the
/// chosen local address. No packets are sent. `None` on a host with no route.
fn lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    Some(socket.local_addr().ok()?.ip())
}

/// Hostname for the QR label; falls back to `computer`.
pub fn default_name() -> String {
    for key in ["ASCUTA_DEVICE_NAME", "HOSTNAME", "COMPUTERNAME"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    if let Ok(value) = std::fs::read_to_string("/etc/hostname") {
        let value = value.trim();
        if !value.is_empty() {
            return value.to_string();
        }
    }
    "computer".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    const TOKEN: &str = "s3cret";

    fn request(method: &str, path: &str, auth: Option<&str>, body: &str) -> HttpRequest {
        HttpRequest {
            method: method.into(),
            path: path.into(),
            authorization: auth.map(str::to_string),
            body: body.into(),
        }
    }

    #[test]
    fn ping_requires_token() {
        let (resp, action) = route(&request("GET", "/v1/ping", None, ""), TOKEN, "pc");
        assert_eq!(resp.status, 401);
        assert!(action.is_none());

        let (resp, action) = route(
            &request("GET", "/v1/ping", Some("Bearer s3cret"), ""),
            TOKEN,
            "pc",
        );
        assert_eq!(resp.status, 200);
        assert!(resp.body.contains("\"name\":\"pc\""));
        assert!(action.is_none());
    }

    #[test]
    fn wrong_and_empty_tokens_denied() {
        assert_eq!(
            route(
                &request("GET", "/v1/ping", Some("Bearer nope"), ""),
                TOKEN,
                "pc"
            )
            .0
            .status,
            401
        );
        // An unconfigured bridge (empty expected token) denies all.
        assert_eq!(
            route(&request("GET", "/v1/ping", Some("Bearer "), ""), "", "pc")
                .0
                .status,
            401
        );
    }

    #[test]
    fn text_post_emits_remote_event() {
        let (resp, action) = route(
            &request(
                "POST",
                "/v1/text",
                Some("Bearer s3cret"),
                r#"{"text":"hello world","enter":true}"#,
            ),
            TOKEN,
            "pc",
        );
        assert_eq!(resp.status, 200);
        assert_eq!(
            action,
            Some(OsEvent::RemoteText {
                text: "hello world".into(),
                enter: true
            })
        );
    }

    #[test]
    fn enter_defaults_false() {
        let (_, action) = route(
            &request("POST", "/v1/text", Some("Bearer s3cret"), r#"{"text":"x"}"#),
            TOKEN,
            "pc",
        );
        assert_eq!(
            action,
            Some(OsEvent::RemoteText {
                text: "x".into(),
                enter: false
            })
        );
    }

    #[test]
    fn empty_or_bad_body_is_rejected() {
        for body in [r#"{"text":""}"#, "not json", r#"{"text":"x","enter":"no"}"#] {
            let (resp, action) = route(
                &request("POST", "/v1/text", Some("Bearer s3cret"), body),
                TOKEN,
                "pc",
            );
            assert_eq!(resp.status, 400, "body: {body}");
            assert!(action.is_none());
        }
    }

    #[test]
    fn unknown_route_404() {
        let (resp, _) = route(
            &request("GET", "/v1/nope", Some("Bearer s3cret"), ""),
            TOKEN,
            "pc",
        );
        assert_eq!(resp.status, 404);
    }

    #[test]
    fn bearer_scheme_is_case_insensitive() {
        assert_eq!(bearer("bearer abc"), Some("abc"));
        assert_eq!(bearer("BEARER abc"), Some("abc"));
        assert_eq!(bearer("Token abc"), None);
        assert_eq!(bearer("abc"), None);
    }

    #[test]
    fn loopback_end_to_end() {
        let (tx, rx) = channel();
        let bridge = Bridge::start("127.0.0.1", 0, TOKEN.into(), "pc".into(), tx).unwrap();
        let base = format!("http://{}", bridge.addr());

        let ping: serde_json::Value =
            serde_json::from_str(&http_get(&format!("{base}/v1/ping"), TOKEN)).unwrap();
        assert_eq!(ping["ok"], true);

        http_post(
            &format!("{base}/v1/text"),
            TOKEN,
            r#"{"text":"from phone","enter":false}"#,
        );

        let entered = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("bridge delivered event");
        assert_eq!(
            entered,
            OsEvent::RemoteText {
                text: "from phone".into(),
                enter: false
            }
        );
    }

    // Minimal HTTP/1.1 client so tests need no extra dev-dependency.
    fn http_get(url: &str, token: &str) -> String {
        raw_request(url, "GET", token, "")
    }

    fn http_post(url: &str, token: &str, body: &str) {
        let _ = raw_request(url, "POST", token, body);
    }

    fn raw_request(url: &str, method: &str, token: &str, body: &str) -> String {
        use std::io::{Read, Write};
        let stripped = url.strip_prefix("http://").unwrap();
        let (authority, path) = stripped.split_once('/').unwrap();
        let mut stream = std::net::TcpStream::connect(authority).unwrap();
        let req = format!(
            "{method} /{path} HTTP/1.1\r\nHost: {authority}\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(req.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response.split("\r\n\r\n").nth(1).unwrap_or("").to_string()
    }

    #[test]
    fn default_name_non_empty() {
        assert!(!default_name().trim().is_empty());
    }
}
