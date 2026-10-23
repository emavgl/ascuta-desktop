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
mod autostart;
mod bridge;
mod cleanup;
mod chord;
#[cfg(feature = "capture")]
mod capture;
mod downloader;
mod focus_check;
mod gui;
mod hotkey;
mod injection;
mod keysource;
#[cfg(target_os = "linux")]
mod layer_bar;
mod models;
#[cfg(feature = "native-ui")]
mod native_ui;
mod overlay;
mod pairing;
#[cfg(target_os = "linux")]
mod portal_keys;
mod readiness;
mod settings;
mod sysfont;
mod state;
mod stt;
mod tray;
mod vad;
mod worker;

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
    /// Final text from the STT worker (empty = nothing to paste).
    Transcript(String),
    /// Model files are now on disk; the STT engine can be started.
    ModelReady,
    /// Result of the optional LLM cleanup (`None` = use the raw text).
    Cleaned {
        original: String,
        cleaned: Option<String>,
    },
    /// Settings GUI saved a change (or deleted a model): reload and apply.
    SettingsChanged,
    /// Settings window "Restart Ascuta": relaunch this program.
    RestartRequested,
    /// The bar's ■ button: stop recording and transcribe.
    StopDictation,
    /// Esc or the bar's X: discard the current recording.
    CancelDictation,
    /// Tray → Settings…: open the GUI in the browser.
    OpenSettings,
}

struct App {
    core: Core,
    settings: settings::Settings,
    overlay: overlay::Bar,
    injector: injection::EnigoInjector,
    tray: Option<tray::SystemTray>,
    rx: mpsc::Receiver<OsEvent>,
    tx: mpsc::Sender<OsEvent>,
    capture: Option<Box<dyn audio::AudioSource>>,
    worker: Option<worker::Worker>,
    cleaner: Option<cleanup::CleanWorker>,
    /// Raw transcript waiting for the cleanup pass, with its deadline.
    pending_clean: Option<std::time::Instant>,
    gui_url: Option<String>,
    gui_base: Option<(String, String)>,
    a11y_enabled_by_us: bool,
    cursor: (f64, f64),
    settings_window: Option<std::process::Child>,
}

impl App {
    /// Open the native settings window (child process); fall back to the
    /// browser page if it cannot be started. At most one window at a time.
    fn open_settings(&mut self) {
        if let Some(child) = &mut self.settings_window {
            if matches!(child.try_wait(), Ok(None)) {
                return; // already open
            }
            self.settings_window = None;
        }
        #[cfg(feature = "native-ui")]
        if let Some((base, token)) = &self.gui_base {
            let spawned = std::env::current_exe().and_then(|exe| {
                std::process::Command::new(exe)
                    .arg("--settings")
                    .env("ASCUTA_GUI_BASE", base)
                    .env("ASCUTA_GUI_TOKEN", token)
                    .spawn()
            });
            match spawned {
                Ok(child) => {
                    self.settings_window = Some(child);
                    return;
                }
                Err(e) => eprintln!("ascuta: native settings window failed: {e}"),
            }
        }
        if let Some(url) = &self.gui_url {
            gui::open_browser(url);
        }
    }

    /// Finish a dictation that produced no text (no speech, no engine, error).
    fn abort_dictation(&mut self) {
        self.core.on_decoded(false);
        self.overlay.hide();
        self.set_tray("idle");
    }

    /// Start (and so warm up) the cleanup model when the feature is on.
    /// Like the Android app: the model loads in the background while the
    /// user is still speaking and then stays resident.
    fn warm_cleaner(&mut self) {
        if self.settings.cleanup_enabled && self.cleaner.is_none() {
            self.cleaner = spawn_cleaner(&self.settings, self.tx.clone());
        }
    }

    /// Hand the transcript to the LLM cleaner when enabled and ready.
    fn try_cleanup(&mut self, text: &str) -> bool {
        if !self.settings.cleanup_enabled {
            return false;
        }
        self.warm_cleaner();
        let sent = self
            .cleaner
            .as_ref()
            .is_some_and(|c| c.submit(text.to_string(), self.settings.language.clone()));
        if sent {
            self.overlay.show_cleaning();
            self.pending_clean =
                Some(std::time::Instant::now() + std::time::Duration::from_secs(30));
        }
        sent
    }

    /// Final step of a dictation: type/copy the text and reset the bar.
    fn finish_transcript(&mut self, text: String) {
        self.core.on_decoded(!text.is_empty());
        if self.core.phase() == Phase::Injecting {
            self.deliver_transcript(&text);
            self.core.on_injected();
        }
        self.overlay.hide();
        self.set_tray("idle");
    }

    /// Type the transcript at the text cursor; when no text field has focus,
    /// copy it to the clipboard and tell the user.
    fn deliver_transcript(&mut self, text: &str) {
        let focus = focus_check::focused_text_field();
        eprintln!("ascuta: focus check: {focus:?}");
        if focus == focus_check::Focus::NoTextField {
            copy_with_notice(&mut self.injector, text);
            return;
        }
        insert_text(&mut self.injector, &self.settings, text);
    }

    fn start_recording(&mut self) {
        eprintln!("ascuta: hotkey pressed");
        if self.core.on_hotkey_pressed() != Phase::Recording {
            return;
        }
        let started = match self.capture.as_mut() {
            Some(c) => c.start().map_err(|e| format!("{e:#}")),
            None => Err("no audio capture backend in this build".into()),
        };
        match started {
            Ok(()) => {
                self.overlay.show_listening();
                self.set_tray("recording");
            }
            Err(e) => {
                eprintln!("ascuta: cannot record: {e}");
                // Recording → Decoding → Idle without a paste.
                self.core.on_hotkey_released();
                self.abort_dictation();
            }
        }
    }

    /// Stop recording and send the speech to the STT worker.
    fn finish_recording(&mut self) {
        eprintln!("ascuta: hotkey released");
        if self.core.on_hotkey_released() != Phase::Decoding {
            return;
        }
        self.overlay.show_decoding();
        self.set_tray("decoding");
        let pcm = self
            .capture
            .as_mut()
            .and_then(|c| c.stop().ok())
            .unwrap_or_default();
        let peak = pcm.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        eprintln!(
            "ascuta: captured {:.1}s of audio, peak {:.3}",
            pcm.len() as f32 / 16_000.0,
            peak as f32 / 32768.0
        );
        match vad::trim(&pcm) {
            None => {
                eprintln!("ascuta: no speech detected");
                self.abort_dictation();
            }
            Some(speech) => {
                let sent = self
                    .worker
                    .as_ref()
                    .is_some_and(|w| w.submit(speech.to_vec()));
                if !sent {
                    eprintln!("ascuta: speech model not ready yet");
                    self.abort_dictation();
                }
            }
        }
    }

    /// Discard the recording in progress (Esc / the bar's X button).
    fn cancel_recording(&mut self) {
        if self.core.phase() != Phase::Recording {
            return;
        }
        eprintln!("ascuta: dictation cancelled");
        if let Some(c) = self.capture.as_mut() {
            let _ = c.stop();
        }
        self.core.on_hotkey_released();
        self.abort_dictation();
    }

    /// Relaunch Ascuta with the settings window open, then exit. The new
    /// process starts after a short delay so the old one has released its
    /// ports, hotkey devices and tray entry.
    fn restart(&mut self) -> ! {
        eprintln!("ascuta: restarting");
        if let Some(mut w) = self.settings_window.take() {
            let _ = w.kill();
        }
        if self.a11y_enabled_by_us {
            focus_check::restore_accessibility();
        }
        if let Ok(exe) = std::env::current_exe() {
            #[cfg(unix)]
            let spawned = std::process::Command::new("sh")
                .arg("-c")
                .arg("sleep 1; exec \"$0\" --gui")
                .arg(&exe)
                .stdin(std::process::Stdio::null())
                .spawn();
            #[cfg(not(unix))]
            let spawned = std::process::Command::new(&exe).arg("--gui").spawn();
            if let Err(e) = spawned {
                eprintln!("ascuta: restart failed: {e}");
            }
        }
        std::process::exit(0)
    }

    fn start_worker(&mut self) {
        if self.worker.is_none() {
            self.worker = spawn_worker(&self.settings, self.tx.clone());
        }
    }

    fn set_tray(&self, status: &str) {
        if let Some(tray) = &self.tray {
            tray.set_status(status);
        }
    }
}

/// Build the STT worker for the configured model, if this build has an
/// engine and the model files are present.
fn spawn_worker(settings: &settings::Settings, tx: mpsc::Sender<OsEvent>) -> Option<worker::Worker> {
    #[cfg(feature = "sherpa")]
    {
        let dir = models::default_models_dir();
        let ready = readiness::dictation_readiness(&dir, &settings.model);
        if ready.code != readiness::ReadinessCode::Ready {
            return None;
        }
        let (model, language) = (settings.model.clone(), settings.language.clone());
        let threads = std::thread::available_parallelism()
            .map(|n| n.get().min(8) as i32)
            .unwrap_or(2);
        Some(worker::Worker::spawn(
            move || {
                let e = worker::sherpa::SherpaTranscriber::load(&dir, &model, &language, threads)?;
                Ok(Box::new(e) as Box<dyn worker::Transcriber>)
            },
            move |text| {
                let _ = tx.send(OsEvent::Transcript(text));
            },
        ))
    }
    #[cfg(not(feature = "sherpa"))]
    {
        let _ = (settings, tx);
        None
    }
}

/// Build the LLM cleanup worker when the LiteRT-LM runtime and the model
/// are installed. The model loads lazily on the worker thread.
fn spawn_cleaner(
    settings: &settings::Settings,
    tx: mpsc::Sender<OsEvent>,
) -> Option<cleanup::CleanWorker> {
    let _ = settings;
    let dir = models::default_models_dir();
    let spec = models::find_model(models::DEFAULT_CLEANUP_MODEL)?;
    if !cleanup::runtime_ready() || !models::missing_files(&dir, spec.id).is_empty() {
        eprintln!("ascuta: cleanup enabled but the runtime/model is not installed");
        return None;
    }
    let path = dir.join(spec.extracted_dir).join(spec.required_files[0]);
    let threads = std::thread::available_parallelism()
        .map(|n| n.get().min(8) as i32)
        .unwrap_or(4);
    Some(cleanup::CleanWorker::spawn(
        move || {
            let c = cleanup::LiteRtCleaner::start(&path, threads)?;
            Ok(Box::new(c) as Box<dyn cleanup::Cleaner>)
        },
        move |original, cleaned| {
            let _ = tx.send(OsEvent::Cleaned { original, cleaned });
        },
    ))
}

/// Download the configured model in the background (Paseo auto-download UX).
fn ensure_model_in_background(
    settings: &settings::Settings,
    tracker: &downloader::Tracker,
    tx: mpsc::Sender<OsEvent>,
) {
    let dir = models::default_models_dir();
    let r = readiness::dictation_readiness(&dir, &settings.model);
    eprintln!("ascuta: {}", r.message);
    if r.code != readiness::ReadinessCode::ModelsMissing {
        return;
    }
    tracker.start(dir, &settings.model, move || {
        let _ = tx.send(OsEvent::ModelReady);
    });
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // Create the bar window lazily on the event-loop thread. It stays
        // hidden while idle (the tray icon shows the app is running).
        self.overlay.ensure_window(event_loop);
        // macOS/Windows: the tray must be created on this thread, now.
        #[cfg(not(target_os = "linux"))]
        if self.tray.is_none() {
            match tray::NativeTray::new() {
                Ok(t) => {
                    t.set_status("idle");
                    self.tray = Some(t);
                }
                Err(e) => eprintln!("ascuta: tray unavailable ({e:#}); continuing without it"),
            }
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        #[cfg(not(target_os = "linux"))]
        if let Some(t) = &self.tray {
            for action in t.poll() {
                let _ = self.tx.send(match action {
                    tray::TrayAction::ShowSettings => OsEvent::OpenSettings,
                    tray::TrayAction::Quit => OsEvent::QuitRequested,
                });
            }
        }
        // Drain OS events (non-blocking) and drive the state machine.
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                OsEvent::QuitRequested => {
                    if self.a11y_enabled_by_us {
                        focus_check::restore_accessibility();
                    }
                    std::process::exit(0)
                }
                OsEvent::HotkeyPressed => self.start_recording(),
                OsEvent::HotkeyReleased => self.finish_recording(),
                OsEvent::CancelDictation => self.cancel_recording(),
                OsEvent::StopDictation => self.finish_recording(),
                OsEvent::Transcript(text) => {
                    eprintln!("ascuta: transcript: {text:?}");
                    if !text.is_empty() && self.try_cleanup(&text) {
                        // Stays in the "decoding" state until the cleaner
                        // answers (or its deadline passes).
                        continue;
                    }
                    self.finish_transcript(text);
                }
                OsEvent::Cleaned { original, cleaned } => {
                    if self.pending_clean.take().is_some() {
                        let text = cleaned.unwrap_or(original);
                        eprintln!("ascuta: cleaned: {text:?}");
                        self.finish_transcript(text);
                    }
                }
                OsEvent::ModelReady => self.start_worker(),
                OsEvent::SettingsChanged => {
                    match settings::Store::load() {
                        Ok(s) => self.settings = s,
                        Err(e) => eprintln!("ascuta: cannot reload settings: {e:#}"),
                    }
                    if let Err(e) = autostart::apply(self.settings.launch_at_login) {
                        eprintln!("ascuta: launch at login: {e:#}");
                    }
                    self.cleaner = None; // model/toggle may differ
                    self.warm_cleaner();
                    // Model may have changed or been deleted: restart engine.
                    self.worker = None;
                    self.start_worker();
                }
                OsEvent::OpenSettings => self.open_settings(),
                OsEvent::RestartRequested => self.restart(),
                OsEvent::RemoteText { text, enter } => {
                    inject_remote(&mut self.injector, &self.settings, &text, enter);
                }
            }
        }
        // Tray/hotkey/bridge/worker threads signal over `rx`, which does not
        // wake winit, so poll at 20 Hz (fast enough for the level meter).
        if let Some(deadline) = self.pending_clean {
            if std::time::Instant::now() > deadline {
                // The model is too slow: should not happen, but never lose
                // the dictation. The late answer is ignored.
                self.pending_clean = None;
                eprintln!("ascuta: cleanup deadline passed; using the raw text");
            }
        }
        match self.core.phase() {
            Phase::Recording => {
                if let Some(c) = &self.capture {
                    self.overlay.set_level(c.volume());
                }
            }
            Phase::Decoding => self.overlay.tick(),
            _ => {}
        }
        _event_loop.set_control_flow(ControlFlow::WaitUntil(
            std::time::Instant::now() + std::time::Duration::from_millis(50),
        ));
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::RedrawRequested => self.overlay.redraw(),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
            }
            WindowEvent::MouseInput {
                state: winit::event::ElementState::Pressed,
                button: winit::event::MouseButton::Left,
                ..
            } => {
                let scale = self.overlay.scale_factor();
                let (lx, ly) = ((self.cursor.0 / scale) as f32, (self.cursor.1 / scale) as f32);
                match overlay::hit_test(self.overlay.mode(), lx, ly) {
                    Some(overlay::Button::Cancel) => self.cancel_recording(),
                    Some(overlay::Button::Stop) => self.finish_recording(),
                    None => {}
                }
            }
            _ => {}
        }
    }
}

/// The winit loop, configured per platform.
/// * Linux: X11/XWayland backend when available (helper windows only).
/// * macOS: "accessory" app — menu-bar item, no Dock icon, never steals
///   focus from the app we paste into.
fn build_event_loop() -> Result<EventLoop<()>> {
    let mut builder = EventLoop::builder();
    #[cfg(target_os = "linux")]
    if std::env::var_os("DISPLAY").is_some() {
        use winit::platform::x11::EventLoopBuilderExtX11;
        builder.with_x11();
    }
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Accessory);
        builder.with_activate_ignoring_other_apps(false);
    }
    Ok(builder.build()?)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    #[cfg(target_os = "linux")]
    if args.iter().any(|a| a == "--key-helper") {
        return keysource::run_helper();
    }
    if args.iter().any(|a| a == "--check") {
        return self_check();
    }
    if let Some(i) = args.iter().position(|a| a == "--transcribe") {
        return transcribe_file(args.get(i + 1).map(String::as_str));
    }
    if let Some(i) = args.iter().position(|a| a == "--cleanup") {
        return cleanup_text(args.get(i + 1).map(String::as_str));
    }
    if let Some(i) = args.iter().position(|a| a == "--debug-bar") {
        return debug_bar(args.get(i + 1).map(String::as_str).unwrap_or("listening"));
    }
    if args.iter().any(|a| a == "--debug-focus") {
        println!("click into a text field (or elsewhere); checking in 4 s...");
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_secs(4));
            println!("focus: {:?}", focus_check::focused_text_field());
        }
        return Ok(());
    }
    if args.iter().any(|a| a == "--debug-mic") {
        return debug_mic();
    }
    if args.iter().any(|a| a == "--debug-keys") {
        return debug_keys();
    }
    if args.iter().any(|a| a == "--settings") {
        return run_settings_window();
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
    let mut gui_pairing: Option<pairing::Pairing> = None;
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
                gui_pairing = Some(pairing.clone());
                eprintln!(
                    "ascuta: phone bridge on {} — scan the QR in the settings page or `ascuta-desktop --pair`",
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
    // not stop the bridge from running, so failures only warn. The icon
    // state (idle/recording/decoding) is the recording indicator now that
    // the always-on-top resting bar is gone.
    // On Linux the StatusNotifier tray is created here; macOS/Windows create
    // theirs inside the event loop (`resumed`), as those platforms require.
    #[cfg(target_os = "linux")]
    let tray = {
        match tray::KsniTray::new(
            {
                let tx = tx.clone();
                move || {
                    let _ = tx.send(OsEvent::QuitRequested);
                }
            },
            {
                let tx = tx.clone();
                move || {
                    let _ = tx.send(OsEvent::OpenSettings);
                }
            },
        ) {
            Ok(tray) => {
                tray.set_status("idle");
                Some(tray)
            }
            Err(e) => {
                eprintln!("ascuta: tray unavailable ({e:#}); continuing without it");
                None
            }
        }
    };
    #[cfg(not(target_os = "linux"))]
    let tray: Option<tray::SystemTray> = None;

    // Hotkey: X11 grab now; Wayland portal binding is the fast-follow
    // (see hotkey.rs). Default: Ctrl+B on Win/Linux. A failed grab (Wayland,
    // missing X11) must not stop the phone bridge either.
    let hotkey_tx = tx.clone();
    let _hotkey_mgr = match hotkey::start_chord_listener(&settings.hotkey, hotkey_tx) {
        Ok(_) => None,
        Err(chord_err) => {
            eprintln!("ascuta: chord hotkey unavailable: {chord_err:#}");
            // Legacy fallback: X11 grab, only for "modifier + key" combos.
            match hotkey::HotkeyManager::register_default(&settings) {
                Ok(manager) => {
                    let hotkey_tx = tx.clone();
                    std::thread::spawn(move || hotkey::pump_events(hotkey_tx));
                    Some(manager)
                }
                Err(e) => {
                    eprintln!("ascuta: hotkey unavailable ({e:#}); phone bridge only");
                    None
                }
            }
        }
    };

    // Injection backend (clipboard paste → keystroke fallback).
    let injector = injection::EnigoInjector::new();

    // Audio capture (feature `capture`) and the STT worker (feature `sherpa`).
    let capture: Option<Box<dyn audio::AudioSource>> = {
        #[cfg(feature = "capture")]
        {
            match capture::CpalCapture::new() {
                Ok(c) => Some(Box::new(c)),
                Err(e) => {
                    eprintln!("ascuta: audio capture unavailable: {e:#}");
                    None
                }
            }
        }
        #[cfg(not(feature = "capture"))]
        {
            eprintln!("ascuta: built without `capture`; local dictation disabled");
            None
        }
    };
    let worker = spawn_worker(&settings, tx.clone());
    let tracker = downloader::Tracker::default();
    if worker.is_none() {
        ensure_model_in_background(&settings, &tracker, tx.clone());
    }

    // Settings GUI: loopback web page; opened from the tray, with `--gui`,
    // or automatically while no model is installed yet.
    let first_run_needs_model = worker.is_none();
    let gui = match pairing::generate_token().and_then(|token| {
        gui::Gui::start(
            gui::Ctx {
                token,
                store: settings::Store::with_path(settings::Store::path()),
                models_dir: models::default_models_dir(),
                tracker: tracker.clone(),
                pairing: gui_pairing,
            },
            gui::DEFAULT_GUI_PORT,
            tx.clone(),
        )
    }) {
        Ok(g) => {
            eprintln!("ascuta: settings at {}", g.url());
            if args.iter().any(|a| a == "--gui") || first_run_needs_model {
                let _ = tx.send(OsEvent::OpenSettings);
            }
            Some(g)
        }
        Err(e) => {
            eprintln!("ascuta: settings GUI unavailable: {e:#}");
            None
        }
    };
    let gui_url = gui.as_ref().map(|g| g.url());
    let gui_base = gui.as_ref().map(|g| (g.base(), g.token().to_string()));

    // Let toolkits expose their text fields so we can tell whether a text
    // cursor exists (restored on quit).
    let a11y_enabled_by_us = focus_check::enable_accessibility();

    // Linux: use the X11/XWayland backend for the HUD when available — a
    // Wayland client cannot place itself or stay on top, and a normal
    // window would steal focus from the app we are about to paste into.
    let event_loop = build_event_loop()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App {
        core: Core::new(),
        settings,
        overlay: overlay::Bar::new({
            let tx = tx.clone();
            move |b| {
                let _ = tx.send(match b {
                    overlay::Button::Cancel => OsEvent::CancelDictation,
                    overlay::Button::Stop => OsEvent::StopDictation,
                });
            }
        }),
        injector,
        tray,
        rx,
        tx,
        capture,
        worker,
        cleaner: None,
        pending_clean: None,
        gui_url,
        gui_base,
        a11y_enabled_by_us,
        cursor: (0.0, 0.0),
        settings_window: None,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// `--transcribe file.wav`: run the configured model on a 16-bit mono PCM
/// WAV (16 kHz recommended) and print the text. Isolates STT from the mic.
fn transcribe_file(path: Option<&str>) -> Result<()> {
    #[cfg(feature = "sherpa")]
    {
        let path = path.ok_or_else(|| anyhow::anyhow!("usage: --transcribe file.wav"))?;
        let bytes = std::fs::read(path)?;
        anyhow::ensure!(bytes.len() > 44 && &bytes[0..4] == b"RIFF", "not a WAV file");
        let rate = u32::from_le_bytes(bytes[24..28].try_into()?);
        let channels = u16::from_le_bytes(bytes[22..24].try_into()?) as usize;
        let data = bytes[44..]
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .step_by(channels.max(1))
            .collect::<Vec<_>>();
        let pcm = if rate == 16_000 {
            data
        } else {
            let f: Vec<f32> = data.iter().map(|&s| s as f32 / 32768.0).collect();
            audio::resample_to_pcm16(&f, rate, 16_000)
        };
        let settings = settings::Store::load()?;
        let dir = models::default_models_dir();
        let samples: Vec<f32> = pcm.iter().map(|&s| s as f32 / 32768.0).collect();
        let mut engine =
            worker::sherpa::SherpaTranscriber::load(&dir, &settings.model, &settings.language, 4)?;
        use worker::Transcriber as _;
        let t0 = std::time::Instant::now();
        let text = engine.transcribe(&samples)?;
        println!(
            "model {} | {:.1}s audio | {:.2}s decode\n{text:?}",
            settings.model,
            samples.len() as f32 / 16_000.0,
            t0.elapsed().as_secs_f32()
        );
        Ok(())
    }
    #[cfg(not(feature = "sherpa"))]
    {
        let _ = path;
        anyhow::bail!("built without the `sherpa` feature")
    }
}

/// `--debug-mic`: record 4 s from the default input and print the levels.
fn debug_mic() -> Result<()> {
    #[cfg(feature = "capture")]
    {
        use audio::AudioSource as _;
        let mut cap = capture::CpalCapture::new()?;
        cap.start()?;
        println!("recording 4 s from the default input device... speak now");
        for _ in 0..8 {
            std::thread::sleep(std::time::Duration::from_millis(500));
            println!("  level {:.3}", cap.volume());
        }
        let pcm = cap.stop()?;
        if let Ok(path) = std::env::var("ASCUTA_DEBUG_WAV") {
            let mut bytes = Vec::with_capacity(44 + pcm.len() * 2);
            let data_len = (pcm.len() * 2) as u32;
            bytes.extend_from_slice(b"RIFF");
            bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
            bytes.extend_from_slice(b"WAVEfmt ");
            bytes.extend_from_slice(&16u32.to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
            bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
            bytes.extend_from_slice(&16_000u32.to_le_bytes());
            bytes.extend_from_slice(&32_000u32.to_le_bytes());
            bytes.extend_from_slice(&2u16.to_le_bytes());
            bytes.extend_from_slice(&16u16.to_le_bytes());
            bytes.extend_from_slice(b"data");
            bytes.extend_from_slice(&data_len.to_le_bytes());
            for s in &pcm {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            std::fs::write(&path, bytes)?;
            println!("wrote {path}");
        }
        let peak = pcm.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        println!(
            "captured {:.1}s, peak {:.3}, vad: {}",
            pcm.len() as f32 / 16_000.0,
            peak as f32 / 32768.0,
            if vad::trim(&pcm).is_some() { "speech" } else { "no speech" }
        );
        Ok(())
    }
    #[cfg(not(feature = "capture"))]
    {
        anyhow::bail!("built without the `capture` feature")
    }
}

/// `--cleanup "text"`: run the LLM cleanup on a string and print the result
/// and timing (benchmark / sanity check for the optional cleanup model).
fn cleanup_text(text: Option<&str>) -> Result<()> {
    use cleanup::Cleaner as _;
    let text = text.ok_or_else(|| anyhow::anyhow!("usage: --cleanup \"text\""))?;
    let settings = settings::Store::load()?;
    let dir = models::default_models_dir();
    let spec = models::find_model(models::DEFAULT_CLEANUP_MODEL).expect("catalog");
    let path = dir.join(spec.extracted_dir).join(spec.required_files[0]);
    let threads = std::thread::available_parallelism()
        .map(|n| n.get().min(8) as i32)
        .unwrap_or(4);
    let t0 = std::time::Instant::now();
    let mut c = cleanup::LiteRtCleaner::start(&path, threads)?;
    println!("model loaded in {:.1}s", t0.elapsed().as_secs_f32());
    let t1 = std::time::Instant::now();
    let raw = c.clean(text, &settings.language)?;
    println!(
        "cleaned in {:.1}s\n  in : {text}\n  out: {raw}\n  use: {:?}",
        t1.elapsed().as_secs_f32(),
        cleanup::accept_cleaned(text, &raw)
    );
    Ok(())
}

/// `--debug-bar [listening|transcribing|cleaning]`: show the real Flow bar
/// window for 25 s (for screenshots / visual review).
fn debug_bar(mode: &str) -> Result<()> {
    struct Demo {
        overlay: overlay::Bar,
        mode: String,
        end: std::time::Instant,
        n: u32,
    }
    impl ApplicationHandler for Demo {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            self.overlay.ensure_window(el);
            match self.mode.as_str() {
                "transcribing" => self.overlay.show_decoding(),
                "cleaning" => self.overlay.show_cleaning(),
                _ => self.overlay.show_listening(),
            }
        }
        fn about_to_wait(&mut self, el: &ActiveEventLoop) {
            if std::time::Instant::now() > self.end {
                el.exit();
                return;
            }
            self.n += 1;
            // Fake voice level so the waveform moves.
            let lvl = 0.5 + 0.5 * ((self.n as f32) * 0.3).sin();
            self.overlay.set_level(lvl);
            self.overlay.tick();
            el.set_control_flow(ControlFlow::WaitUntil(
                std::time::Instant::now() + std::time::Duration::from_millis(50),
            ));
        }
        fn window_event(&mut self, _: &ActiveEventLoop, _: winit::window::WindowId, ev: WindowEvent) {
            if let WindowEvent::RedrawRequested = ev {
                self.overlay.redraw();
            }
        }
    }
    let el = build_event_loop()?;
    let mut demo = Demo {
        overlay: overlay::Bar::new(|_| {}),
        mode: mode.to_string(),
        end: std::time::Instant::now() + std::time::Duration::from_secs(25),
        n: 0,
    };
    el.run_app(&mut demo)?;
    Ok(())
}

/// `--debug-keys`: print the keyboards being watched and every raw key
/// event for 20 s, plus hotkey press/release as the tracker sees them.
fn debug_keys() -> Result<()> {
    let settings = settings::Store::load()?;
    let chord = chord::Chord::parse(&settings.hotkey).map_err(|e| anyhow::anyhow!(e))?;
    let (tx, rx) = mpsc::channel();
    let n = keysource::start(tx)?;
    println!("watching {n} keyboard(s); hotkey = {chord}. Press keys (20 s)...");
    let mut tracker = chord::Tracker::new(chord);
    let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
        if let Ok(raw) = rx.recv_timeout(left) {
            println!("key {:<12} {}", raw.key, if raw.down { "down" } else { "up" });
            match tracker.on_key(raw.key, raw.down) {
                Some(true) => println!("  >>> HOTKEY PRESSED"),
                Some(false) => println!("  >>> HOTKEY RELEASED"),
                None => {}
            }
        }
    }
    Ok(())
}

/// `--settings`: the native window, started by the core with the loopback
/// address and session token in the environment (not on the command line).
fn run_settings_window() -> Result<()> {
    #[cfg(feature = "native-ui")]
    {
        let base = std::env::var("ASCUTA_GUI_BASE")
            .map_err(|_| anyhow::anyhow!("ASCUTA_GUI_BASE not set; start Ascuta and use the tray"))?;
        let token = std::env::var("ASCUTA_GUI_TOKEN").unwrap_or_default();
        native_ui::run(base, token)
    }
    #[cfg(not(feature = "native-ui"))]
    {
        anyhow::bail!("built without the `native-ui` feature")
    }
}

/// When nothing editable has focus, keep the transcript in the clipboard
/// and tell the user, instead of sending a paste keystroke to a random app.
fn copy_with_notice(
    injector: &mut impl injection::Injector,
    text: &str,
) {
    match injector.copy_only(text) {
        Ok(()) => injection::show_notice(
            "Copied to clipboard",
            "No text field was selected. Paste with Ctrl+V.",
        ),
        Err(e) => eprintln!("ascuta: clipboard failed: {e:#}"),
    }
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
    let focus = focus_check::focused_text_field();
    eprintln!("ascuta: remote focus check: {focus:?}");
    inject_remote_with_focus(injector, settings, text, enter, focus);
}

/// The focus-checked half of [`inject_remote`], split out so tests can drive
/// both the paste and the clipboard-only branch with the fake injector.
fn inject_remote_with_focus(
    injector: &mut impl injection::Injector,
    settings: &settings::Settings,
    text: &str,
    enter: bool,
    focus: focus_check::Focus,
) {
    if text.is_empty() {
        return;
    }
    if focus == focus_check::Focus::NoTextField {
        copy_with_notice(injector, text);
        return;
    }
    insert_text(injector, settings, text);
    if enter {
        if let Err(e) = injector.press_enter() {
            eprintln!("ascuta: remote Enter failed: {e:#}");
        }
    }
    eprintln!("ascuta: injected {} chars from phone", text.chars().count());
}

/// Insert a transcript according to the configured mode. Typing is the
/// default; if it fails (no key backend, missing Wayland portal) fall back
/// to the clipboard + paste path so the dictation is not lost. A trailing
/// space keeps consecutive dictations (local or from the phone) apart.
fn insert_text(
    injector: &mut impl injection::Injector,
    settings: &settings::Settings,
    text: &str,
) {
    let text = injection::with_trailing_space(text);
    if text.is_empty() {
        return;
    }
    if settings.injection_mode == "type" {
        match injector.type_text(&text) {
            Ok(()) => return,
            Err(e) => eprintln!("ascuta: typing failed ({e:#}); falling back to paste"),
        }
    }
    if let Err(e) = injector.paste_text(&text, settings.preserve_clipboard) {
        eprintln!("ascuta: injection failed: {e:#}");
    }
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

    // VAD gate + worker round trip (fake engine): silence aborts, speech
    // is transcribed and tidied.
    {
        assert!(vad::trim(&vec![0i16; 16_000]).is_none());
        let burst: Vec<i16> = (0..8_000)
            .map(|i| ((i as f32 * 0.2).sin() * 9_000.0) as i16)
            .collect();
        let speech = vad::trim(&burst).expect("tone is speech");
        let (rtx, rrx) = mpsc::channel();
        let w = worker::Worker::spawn(
            || {
                Ok(Box::new(worker::FakeTranscriber {
                    reply: "hello".into(),
                }) as Box<dyn worker::Transcriber>)
            },
            move |t| {
                let _ = rtx.send(t);
            },
        );
        assert!(w.submit(speech.to_vec()));
        let got = rrx.recv_timeout(std::time::Duration::from_secs(2))?;
        assert_eq!(got, "Hello.");
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

        inject_remote_with_focus(
            &mut injector,
            &settings,
            "from phone",
            true,
            focus_check::Focus::TextField,
        );
        assert_eq!(injector.enters(), 1);
        assert_eq!(injector.typed(), &["from phone ".to_string()]);
        assert_eq!(injector.pasted().len(), 1);

        // Paste mode uses the clipboard path instead.
        let mut paste_settings = settings.clone();
        paste_settings.injection_mode = "paste".into();
        inject_remote_with_focus(
            &mut injector,
            &paste_settings,
            "pasted text",
            false,
            focus_check::Focus::TextField,
        );
        assert_eq!(injector.pasted().last(), Some(&"pasted text ".to_string()));

        // No focused text field: keep the transcript in the clipboard and
        // never send a paste/Enter keystroke to whatever window is in front.
        inject_remote_with_focus(
            &mut injector,
            &settings,
            "copied only",
            true,
            focus_check::Focus::NoTextField,
        );
        assert_eq!(injector.enters(), 1);
        assert_eq!(injector.pasted().len(), 2);
        assert_eq!(injector.typed().len(), 1);
        assert_eq!(injector.copied(), &["copied only".to_string()]);

        let mut tokenized = settings.clone();
        assert!(tokenized.ensure_bridge_token());
        assert_eq!(tokenized.bridge_token.len(), 64);
    }

    store.save(&settings)?;
    let reloaded = store.read()?;
    assert_eq!(reloaded, settings);

    println!("ascuta --check: OK (press/release/vad/worker/inject/phone-bridge/settings-roundtrip)");
    Ok(())
}
