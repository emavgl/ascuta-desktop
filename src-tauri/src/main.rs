//! ascuta-desktop — M0 scaffold.
//!
//! Hybrid split (§3/§4/§8): this Rust binary owns tray, HUD, hotkey,
//! audio/STT (later), state, injection. The Tauri webview (added in
//! `src/` + `tauri.conf.json`) is settings/history window ONLY.
//!
//! `--check` runs a headless self-test with fake tray/hotkey/overlay/
//! injector: the CI gate for machines without a display server.
//!
//! Headless-CI contract: `cargo test` + `cargo run -- --check` must pass
//! with no display server, no audio device, no network, and no downloaded
//! models. Native backends (tray/hotkey/overlay/clipboard) degrade or stay
//! behind fakes; not-yet-wired milestone APIs are `allow(dead_code)` until
//! their worker lands rather than breaking the gate.

#![allow(dead_code)]

mod audio;
mod bridge;
mod downloader;
mod hotkey;
mod injection;
mod models;
mod overlay;
mod pairing;
mod readiness;
mod settings;
mod state;
mod stt;
mod tray;

use std::sync::mpsc;

use anyhow::Result;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};

use injection::Injector as _;
use overlay::Overlay as _;
use state::{Core, Phase};
use tray::Tray as _;

/// Events arriving from OS-integration threads into the winit loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OsEvent {
    HotkeyPressed,
    HotkeyReleased,
    QuitRequested,
    /// Transcript pushed by the paired phone over the LAN bridge.
    RemoteText {
        text: String,
        enter: bool,
    },
}

struct App {
    core: Core,
    settings: settings::Settings,
    overlay: overlay::WinitOverlay,
    injector: injection::EnigoInjector,
    rx: mpsc::Receiver<OsEvent>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // Create the HUD pill window lazily on the event-loop thread.
        self.overlay.ensure_window(event_loop);
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Drain OS events (non-blocking) and drive the state machine.
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                OsEvent::QuitRequested => std::process::exit(0),
                OsEvent::HotkeyPressed => {
                    self.core.on_hotkey_pressed();
                    self.overlay.show_listening();
                }
                OsEvent::HotkeyReleased => {
                    if self.core.on_hotkey_released() == Phase::Decoding {
                        self.overlay.show_decoding();
                        // M0: no STT yet — the "transcript" is the stub.
                        // M2 replaces this with the worker-thread decode.
                        // Owned String (not a literal) so the emptiness
                        // check below stays a real branch for clippy.
                        let text = String::from("hello");
                        self.core.on_decoded(!text.is_empty());
                        if self.core.phase() == Phase::Injecting {
                            if let Err(e) = self
                                .injector
                                .paste_text(&text, self.settings.preserve_clipboard)
                            {
                                eprintln!("ascuta: injection failed: {e:#}");
                            }
                            self.core.on_injected();
                        }
                        self.overlay.hide();
                    }
                }
                OsEvent::RemoteText { text, enter } => {
                    inject_remote(&mut self.injector, &self.settings, &text, enter);
                }
            }
        }
        // Poll at a low cadence; real wakeups come from winit user events.
        // (tray + hotkey crates run their own threads and signal via `rx`.)
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        _event: WindowEvent,
    ) {
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--check") {
        return self_check();
    }
    if args.iter().any(|a| a == "--pair" || a == "--serve") {
        return pair_and_serve();
    }

    let mut settings = settings::Store::load()?;
    if settings.ensure_bridge_token() {
        // Persist once so the QR keeps working across restarts.
        settings::Store::with_path(settings::Store::path()).save(&settings)?;
    }
    eprintln!("ascuta: settings at {}", settings::Store::path().display());

    let (tx, rx) = mpsc::channel::<OsEvent>();

    // Phone bridge (opt-in, PLAN §12): receives transcripts from the paired
    // Android app and replays them as `RemoteText` into the same loop.
    let _bridge = if settings.bridge_enabled {
        let name = bridge::default_name();
        match bridge::Bridge::start(
            "0.0.0.0",
            settings.bridge_port,
            settings.bridge_token.clone(),
            name.clone(),
            tx.clone(),
        ) {
            Ok(b) => {
                let pairing = b.pairing(&settings.bridge_token, &name);
                eprintln!(
                    "ascuta: phone bridge on {} — scan the QR printed by `ascuta-desktop --pair`",
                    b.addr()
                );
                eprintln!("{}", pairing.to_uri());
                Some(b)
            }
            Err(e) => {
                eprintln!("ascuta: phone bridge disabled: {e:#}");
                None
            }
        }
    } else {
        None
    };

    // Tray (native StatusNotifier via ksni — NOT Tauri tray, see PLAN §8).
    // Missing tray host (e.g. GNOME without the AppIndicator extension) must
    // not stop the bridge from running, so failures only warn.
    let _tray = match tray::KsniTray::new({
        let tx = tx.clone();
        move || {
            let _ = tx.send(OsEvent::QuitRequested);
        }
    }) {
        Ok(tray) => {
            tray.set_status("idle");
            Some(tray)
        }
        Err(e) => {
            eprintln!("ascuta: tray unavailable ({e:#}); continuing without it");
            None
        }
    };

    // Hotkey: X11 grab now; Wayland portal binding is the fast-follow
    // (see hotkey.rs). Default: Ctrl+B on Win/Linux. A failed grab (Wayland,
    // missing X11) must not stop the phone bridge either.
    let _hotkey_mgr = match hotkey::HotkeyManager::register_default(&settings) {
        Ok(manager) => {
            std::thread::spawn(move || hotkey::pump_events(tx));
            Some(manager)
        }
        Err(e) => {
            eprintln!("ascuta: hotkey unavailable ({e:#}); continuing with the phone bridge only");
            None
        }
    };

    // Injection backend (clipboard paste → keystroke fallback).
    let injector = injection::EnigoInjector::new();

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App {
        core: Core::new(),
        settings,
        overlay: overlay::WinitOverlay::new(),
        injector,
        rx,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// Inject a transcript that arrived from the paired phone. Same path as a
/// locally transcribed result; never panics on a headless/odd session.
fn inject_remote(
    injector: &mut impl injection::Injector,
    settings: &settings::Settings,
    text: &str,
    enter: bool,
) {
    if text.is_empty() {
        return;
    }
    if let Err(e) = injector.paste_text(text, settings.preserve_clipboard) {
        eprintln!("ascuta: remote injection failed: {e:#}");
        return;
    }
    if enter {
        if let Err(e) = injector.press_enter() {
            eprintln!("ascuta: remote Enter failed: {e:#}");
        }
    }
    eprintln!("ascuta: injected {} chars from phone", text.chars().count());
}

/// `ascuta-desktop --pair`: enable the bridge, print the QR, serve until
/// Ctrl+C. The minimum setup to try the phone flow on a machine with no
/// tray/HUD (and no Tauri/webview).
fn pair_and_serve() -> Result<()> {
    let mut settings = settings::Store::load()?;
    settings.bridge_enabled = true;
    settings.ensure_bridge_token();
    settings::Store::with_path(settings::Store::path()).save(&settings)?;

    let name = bridge::default_name();
    let (tx, rx) = mpsc::channel::<OsEvent>();
    let bridge = bridge::Bridge::start(
        "0.0.0.0",
        settings.bridge_port,
        settings.bridge_token.clone(),
        name.clone(),
        tx,
    )?;
    let pairing = bridge.pairing(&settings.bridge_token, &name);

    println!("ascuta phone bridge listening on {}", bridge.addr());
    println!();
    println!("{}", pairing.render_unicode()?);
    println!("{}", pairing.to_uri());
    println!();
    println!("Scan with Ascuta on Android → Settings → Send to computer.");
    println!("Press Ctrl+C to stop.");

    let mut injector = injection::EnigoInjector::new();
    while let Ok(event) = rx.recv() {
        if let OsEvent::RemoteText { text, enter } = event {
            inject_remote(&mut injector, &settings, &text, enter);
        }
    }
    Ok(())
}

/// Headless CI gate: drive the state machine + fake backends end to end.
/// Runs with no display server, no tray, no webview.
fn self_check() -> Result<()> {
    let dir = std::env::temp_dir().join(format!("ascuta-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let store = settings::Store::with_path(dir.join("settings.json"));
    let settings = store.read()?;
    assert!(settings.preserve_clipboard, "default preserve_clipboard");

    let mut core = Core::new();
    let mut overlay = overlay::FakeOverlay::new();
    let mut injector = injection::FakeInjector::new();

    // M1/M2 core-logic gates (pure Rust, no display/audio/native needed):
    // model catalog default, audio DSP helpers, STT assembly + tidy.
    assert!(models::is_known_model(models::DEFAULT_STT_MODEL));
    assert!(models::is_known_model(&settings.model));
    let pcm = audio::resample_to_pcm16(&[0.0, 0.5, -0.5], 16_000, 16_000);
    assert_eq!(pcm.len(), 3);
    assert_eq!(audio::rms_volume(&[0.0; 64]), 0.0);
    {
        use std::collections::{HashMap, HashSet};
        let committed = vec!["s1".to_string()];
        let mut tx = HashMap::new();
        tx.insert("s1".to_string(), "hello world".to_string());
        let finals: HashSet<String> = ["s1".to_string()].into();
        assert_eq!(
            stt::assemble_transcript(&committed, &tx, &finals),
            "hello world"
        );
        assert_eq!(stt::tidy_transcript("hello world"), "Hello world.");
        assert_eq!(stt::apply_confidence_gate("x", Some(-5.0)), "");
    }
    // Readiness + download plan for the configured model (files absent in
    // CI → ModelsMissing with a concrete plan; Ready on real desktops).
    {
        let dir = models::default_models_dir();
        let r = readiness::dictation_readiness(&dir, &settings.model);
        eprintln!("ascuta: models at {} — {}", dir.display(), r.message);
        let plan = downloader::plan_for(&dir, &settings.model).expect("known model");
        assert_eq!(plan.archive, downloader::ArchiveKind::TarBz2);
        let _ = r.retryable;
    }

    // Simulate hold-to-talk: press → release with a (stub) transcript.
    core.on_hotkey_pressed();
    assert_eq!(core.phase(), Phase::Recording);
    overlay.show_listening();
    assert!(overlay.visible());

    assert_eq!(core.on_hotkey_released(), Phase::Decoding);
    overlay.show_decoding();
    core.on_decoded(true);
    assert_eq!(core.phase(), Phase::Injecting);
    injector.paste_text("hello", true)?;
    core.on_injected();
    assert_eq!(core.phase(), Phase::Idle);
    overlay.hide();
    assert!(!overlay.visible());

    assert_eq!(injector.pasted(), &["hello".to_string()]);

    // Phone bridge (PLAN §12): pure routing + pairing + token, no sockets
    // (the socket path is covered by `bridge::tests::loopback_end_to_end`).
    {
        use bridge::{route, HttpRequest};
        let bridge_token = "abc";
        let ping = route(
            &HttpRequest {
                method: "GET".into(),
                path: "/v1/ping".into(),
                authorization: Some("Bearer abc".into()),
                body: String::new(),
            },
            bridge_token,
            "ci-pc",
        );
        assert_eq!(ping.0.status, 200);
        let denied = route(
            &HttpRequest {
                method: "GET".into(),
                path: "/v1/ping".into(),
                authorization: None,
                body: String::new(),
            },
            bridge_token,
            "ci-pc",
        );
        assert_eq!(denied.0.status, 401);
        let text = route(
            &HttpRequest {
                method: "POST".into(),
                path: "/v1/text".into(),
                authorization: Some("Bearer abc".into()),
                body: r#"{"text":"from phone","enter":true}"#.into(),
            },
            bridge_token,
            "ci-pc",
        );
        assert_eq!(
            text.1,
            Some(OsEvent::RemoteText {
                text: "from phone".into(),
                enter: true
            })
        );

        let pairing = pairing::Pairing {
            host: "10.0.0.5".into(),
            port: 8737,
            token: bridge_token.into(),
            name: "ci-pc".into(),
        };
        assert_eq!(pairing::Pairing::parse_uri(&pairing.to_uri())?, pairing);
        assert!(pairing.render_unicode()?.contains('█') || pairing.render_unicode()?.contains('▀'));

        inject_remote(&mut injector, &settings, "from phone", true);
        assert_eq!(injector.enters(), 1);
        assert_eq!(injector.pasted().len(), 2);

        let mut tokenized = settings.clone();
        assert!(tokenized.ensure_bridge_token());
        assert_eq!(tokenized.bridge_token.len(), 64);
    }

    store.save(&settings)?;
    let reloaded = store.read()?;
    assert_eq!(reloaded, settings);

    println!("ascuta --check: OK (press/release/paste-hello/phone-bridge/settings-roundtrip)");
    Ok(())
}
