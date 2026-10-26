//! Native settings window (egui/eframe on winit — no webview, like
//! PhotoCraft). Runs as a child process (`ascuta-desktop --settings`) so its
//! event loop never competes with the HUD/tray loop of the core, and talks
//! to the core over the same authenticated loopback API as the web page.
//! Layout and palette mirror the Android app (sections of title/subtitle
//! rows, "Fresh Sky" colors).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use eframe::egui::{self, Color32, RichText};
use serde_json::{json, Value};

const LANGS: &[&str] = &[
    "auto", "en", "it", "de", "fr", "es", "pt", "nl", "pl", "ru", "ja", "zh",
];

const PRIMARY: Color32 = Color32::from_rgb(0x05, 0x82, 0xca);
const ERROR: Color32 = Color32::from_rgb(0xba, 0x1a, 0x1a);

#[derive(Default)]
struct Shared {
    state: Option<Value>,
    connected: bool,
    toast: Option<(String, Instant)>,
}

#[derive(Clone)]
struct Api {
    base: String,
    token: String,
}

impl Api {
    fn get_state(&self) -> Result<Value> {
        let s = ureq::get(&format!("{}/api/state", self.base))
            .set("X-Ascuta-Token", &self.token)
            .timeout(Duration::from_secs(3))
            .call()?
            .into_string()?;
        Ok(serde_json::from_str(&s)?)
    }

    /// POST and return the error message, if any (server `error` field).
    fn post(&self, path: &str, body: &Value) -> Result<(), String> {
        match ureq::post(&format!("{}{path}", self.base))
            .set("X-Ascuta-Token", &self.token)
            .timeout(Duration::from_secs(5))
            .send_string(&body.to_string())
        {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(_, r)) => {
                let msg = r
                    .into_string()
                    .ok()
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                    .and_then(|v| v.get("error").and_then(Value::as_str).map(String::from));
                Err(msg.unwrap_or_else(|| "request failed".into()))
            }
            Err(e) => Err(e.to_string()),
        }
    }
}

struct SettingsApp {
    api: Api,
    shared: Arc<Mutex<Shared>>,
    ctx: egui::Context,
    alive: Arc<AtomicBool>,
    recording_hotkey: bool,
    /// Raw global key events for the recorder (modifier-only chords).
    key_rx: Option<std::sync::mpsc::Receiver<crate::keysource::RawKey>>,
    rec_pressed: std::collections::HashSet<&'static str>,
    rec_seen: Vec<&'static str>,
    confirm_delete: Option<String>,
    port: u16,
}

impl SettingsApp {
    fn new(cc: &eframe::CreationContext<'_>, api: Api) -> Self {
        apply_theme(&cc.egui_ctx);
        let shared = Arc::new(Mutex::new(Shared::default()));
        let alive = Arc::new(AtomicBool::new(true));
        {
            let (api, shared, alive, ctx) =
                (api.clone(), shared.clone(), alive.clone(), cc.egui_ctx.clone());
            std::thread::spawn(move || {
                while alive.load(Ordering::Relaxed) {
                    let res = api.get_state();
                    {
                        let mut s = shared.lock().unwrap();
                        match res {
                            Ok(v) => {
                                s.state = Some(v);
                                s.connected = true;
                            }
                            Err(_) => s.connected = false,
                        }
                    }
                    ctx.request_repaint();
                    std::thread::sleep(Duration::from_millis(700));
                }
            });
        }
        Self {
            api,
            shared,
            ctx: cc.egui_ctx.clone(),
            alive,
            recording_hotkey: false,
            key_rx: None,
            rec_pressed: Default::default(),
            rec_seen: Vec::new(),
            confirm_delete: None,
            port: 0,
        }
    }

    /// Fire-and-forget POST; the poller picks up the new state.
    fn post(&self, path: &'static str, body: Value, ok_msg: &'static str) {
        let (api, shared, ctx) = (self.api.clone(), self.shared.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let msg = match api.post(path, &body) {
                Ok(()) => ok_msg.to_string(),
                Err(e) => e,
            };
            if !msg.is_empty() {
                shared.lock().unwrap().toast = Some((msg, Instant::now()));
            }
            ctx.request_repaint();
        });
    }
}

impl Drop for SettingsApp {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}

const ON_PRIMARY_CONTAINER: Color32 = Color32::from_rgb(0x00, 0x35, 0x54);
const PRIMARY_CONTAINER: Color32 = Color32::from_rgb(0xd9, 0xf2, 0xfe);
const SURFACE_VARIANT: Color32 = Color32::from_rgb(0xdf, 0xe2, 0xeb);
const ON_SURFACE: Color32 = Color32::from_rgb(0x1a, 0x1b, 0x20);
const ON_SURFACE_VARIANT: Color32 = Color32::from_rgb(0x43, 0x47, 0x4e);
const OUTLINE: Color32 = Color32::from_rgb(0x73, 0x77, 0x7f);

fn system_font() -> Option<Vec<u8>> {
    crate::sysfont::bytes()
}

fn apply_theme(ctx: &egui::Context) {
    // Always the light, white-background look (matches the web page).
    ctx.options_mut(|o| o.theme_preference = egui::ThemePreference::Light);

    if let Some(bytes) = system_font() {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "system-ui".into(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            list.insert(0, "system-ui".into());
        }
        ctx.set_fonts(fonts);
    }

    let mut v = egui::Visuals::light();
    v.panel_fill = Color32::WHITE;
    v.window_fill = Color32::WHITE;
    v.extreme_bg_color = Color32::WHITE;
    v.faint_bg_color = SURFACE_VARIANT;
    v.override_text_color = Some(ON_SURFACE);
    v.selection.bg_fill = PRIMARY;
    v.hyperlink_color = PRIMARY;
    v.widgets.noninteractive.bg_stroke.color = SURFACE_VARIANT;
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, OUTLINE);
    v.widgets.inactive.weak_bg_fill = Color32::WHITE;
    v.widgets.hovered.bg_stroke.color = PRIMARY;
    v.widgets.hovered.weak_bg_fill = PRIMARY_CONTAINER;
    v.widgets.active.weak_bg_fill = PRIMARY_CONTAINER;
    ctx.set_visuals_of(egui::Theme::Light, v);

    // Larger, desktop-sized text and roomier controls.
    ctx.all_styles_mut(|style| {
        use egui::TextStyle::*;
        for (ts, font) in style.text_styles.iter_mut() {
            font.size = match ts {
                Heading => 30.0,
                Body | Button => 17.0,
                Small => 14.5,
                Monospace => 15.0,
                _ => 17.0,
            };
        }
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(16.0, 8.0);
        style.spacing.interact_size.y = 36.0;
        style.spacing.icon_width = 22.0;
        style.spacing.icon_width_inner = 14.0;
    });
}

fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(Color32::WHITE))
            .fill(PRIMARY)
            .corner_radius(20.0),
    )
}

fn text_button(ui: &mut egui::Ui, text: RichText) -> egui::Response {
    ui.add(egui::Button::new(text).frame(false))
}

fn accent(_ui: &egui::Ui) -> Color32 {
    PRIMARY
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(18.0);
    ui.label(
        RichText::new(title.to_uppercase())
            .small()
            .strong()
            .color(accent(ui)),
    );
    ui.add_space(2.0);
}

/// Below this width the control drops under the text instead of sitting
/// on the right (phone-width / narrow windows).
const NARROW: f32 = 520.0;

/// Title + subtitle on the left, `control` on the right (Android row);
/// stacked on narrow windows.
fn row(ui: &mut egui::Ui, title: &str, subtitle: &str, control: impl FnOnce(&mut egui::Ui)) {
    let narrow = ui.available_width() < NARROW;
    let text = |ui: &mut egui::Ui| {
        ui.label(RichText::new(title));
        if !subtitle.is_empty() {
            ui.label(RichText::new(subtitle).small().color(ON_SURFACE_VARIANT));
        }
    };
    if narrow {
        ui.vertical(|ui| {
            text(ui);
            ui.horizontal_wrapped(control);
        });
    } else {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_max_width((ui.available_width() - 210.0).max(180.0));
                text(ui);
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), control);
        });
    }
    ui.add_space(2.0);
    ui.separator();
}

fn mb(n: u64) -> u64 {
    n / 1_048_576
}

/// `Ctrl+Shift+B` style string from an egui key event (None for pure
/// modifiers / unsupported keys). The core validates it on save.
fn hotkey_string(key: egui::Key, m: egui::Modifiers) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if m.ctrl {
        parts.push("Ctrl");
    }
    if m.alt {
        parts.push("Alt");
    }
    if m.shift {
        parts.push("Shift");
    }
    if m.mac_cmd {
        parts.push("Super");
    }
    let name = key.name();
    let mut s = parts.join("+");
    if !s.is_empty() {
        s.push('+');
    }
    s.push_str(name);
    s
}

fn draw_qr(ui: &mut egui::Ui, uri: &str) {
    let Ok(code) = qrcode::QrCode::new(uri.as_bytes()) else {
        return;
    };
    let n = code.width();
    let cell = 5.0;
    let quiet = 2usize;
    let side = (n + quiet * 2) as f32 * cell;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 6.0, Color32::WHITE);
    for (i, c) in code.to_colors().iter().enumerate() {
        if *c == qrcode::Color::Dark {
            let (x, y) = ((i % n + quiet) as f32, (i / n + quiet) as f32);
            let r = egui::Rect::from_min_size(
                rect.min + egui::vec2(x * cell, y * cell),
                egui::vec2(cell, cell),
            );
            p.rect_filled(r, 0.0, Color32::BLACK);
        }
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let (state, connected, toast) = {
            let mut s = self.shared.lock().unwrap();
            if s.toast.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(3)) {
                s.toast = None;
            }
            (
                s.state.clone(),
                s.connected,
                s.toast.as_ref().map(|(m, _)| m.clone()),
            )
        };

        // Hotkey capture happens before layout so the key never reaches widgets.
        if self.recording_hotkey && self.key_rx.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(40));
            let mut done: Option<Vec<&'static str>> = None;
            if let Some(rx) = &self.key_rx {
                while let Ok(raw) = rx.try_recv() {
                    if raw.down {
                        self.rec_pressed.insert(raw.key);
                        if !self.rec_seen.contains(&raw.key) {
                            self.rec_seen.push(raw.key);
                        }
                    } else {
                        self.rec_pressed.remove(raw.key);
                    }
                    // The chord is complete when everything was released.
                    if self.rec_pressed.is_empty() && !self.rec_seen.is_empty() {
                        done = Some(std::mem::take(&mut self.rec_seen));
                    }
                }
            }
            if let Some(keys) = done {
                self.recording_hotkey = false;
                if keys.as_slice() != ["Escape"] {
                    if let Some(spec) = crate::chord::chord_from_keys(&keys) {
                        self.post(
                            "/api/settings",
                            json!({"hotkey": spec}),
                            "Hotkey saved. Press Restart to apply",
                        );
                    }
                }
            }
        } else if self.recording_hotkey {
            let events = ui.input(|i| i.events.clone());
            for ev in events {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = ev
                {
                    self.recording_hotkey = false;
                    if key != egui::Key::Escape {
                        self.post(
                            "/api/settings",
                            json!({"hotkey": hotkey_string(key, modifiers)}),
                            "Hotkey saved — restart Ascuta to apply",
                        );
                    }
                }
            }
        }

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let side = ((ui.available_width() - 720.0) / 2.0).max(8.0);
                ui.set_max_width(ui.available_width() - side);
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| ui.add_space(side));
                ui.heading("Ascuta Desktop");
                ui.label(RichText::new("Private dictation, fully offline. No account, no cloud.").color(ON_SURFACE_VARIANT));
                ui.add_space(8.0);

                let Some(st) = state else {
                    ui.label(if connected {
                        "Loading..."
                    } else {
                        "Cannot reach Ascuta. Is the app running?"
                    });
                    return;
                };
                if !connected {
                    ui.colored_label(ERROR, "Connection to Ascuta lost, retrying...");
                }
                self.body(ui, &st);
            });
        });

        if let Some(model) = self.confirm_delete.clone() {
            egui::Window::new("Delete model?")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.label(format!("Delete {model}? This frees the disk space."));
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            self.post("/api/delete", json!({"model": model}), "Deleted");
                            self.confirm_delete = None;
                        }
                        if ui.button("Cancel").clicked() {
                            self.confirm_delete = None;
                        }
                    });
                });
        }

        if let Some(msg) = toast {
            egui::Area::new(egui::Id::new("toast"))
                .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -16.0])
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| ui.label(msg));
                });
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
    }
}

impl SettingsApp {
    fn body(&mut self, ui: &mut egui::Ui, st: &Value) {
        let s = &st["settings"];
        let str_of = |v: &Value| v.as_str().unwrap_or("").to_string();
        let model = str_of(&s["model"]);
        let ready = st["readiness"]["ready"].as_bool().unwrap_or(false);

        // Banner
        egui::Frame::new()
            .fill(if ready { PRIMARY_CONTAINER } else { SURFACE_VARIANT })
            .corner_radius(16.0)
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let fg = if ready { ON_PRIMARY_CONTAINER } else { ON_SURFACE };
                ui.label(
                    RichText::new(if ready { "All set" } else { "Setup needed" })
                        .size(22.0)
                        .strong()
                        .color(fg),
                );
                ui.label(
                    RichText::new(if ready {
                        "Hold the hotkey, speak, release: the text is typed where your cursor is."
                            .to_string()
                    } else {
                        str_of(&st["readiness"]["message"])
                    })
                    .color(fg),
                );
            });

        match st["keyboard"].as_str().unwrap_or("ok") {
            "needs_group" => {
                ui.add_space(8.0);
                egui::Frame::new()
                    .fill(SURFACE_VARIANT)
                    .corner_radius(16.0)
                    .inner_margin(16.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new("Enable the global hotkey").strong());
                        ui.label(
                            "Ascuta needs permission to read key presses to detect the \
                             hold-to-talk hotkey (adds you to the \"input\" group).",
                        );
                        if primary_button(ui, "Enable keyboard access").clicked() {
                            self.post(
                                "/api/enable_keyboard",
                                json!({}),
                                "Done. Log out and back in, then restart Ascuta",
                            );
                        }
                    });
            }
            "needs_relogin" => {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "Keyboard access granted. Log out and back in, then restart Ascuta.",
                    )
                    .color(ON_SURFACE_VARIANT),
                );
            }
            _ => {}
        }

        section(ui, "Current configuration");
        row(ui, "Voice model", &model, |_| {});
        let mut lang = str_of(&s["language"]);
        let before = lang.clone();
        row(ui, "Language", "auto-detect or a fixed language", |ui| {
            egui::ComboBox::from_id_salt("lang")
                .selected_text(&lang)
                .show_ui(ui, |ui| {
                    for l in LANGS {
                        ui.selectable_value(&mut lang, l.to_string(), *l);
                    }
                });
        });
        if lang != before {
            self.post("/api/settings", json!({"language": lang}), "Saved");
        }

        section(ui, "Common");
        let hotkey = str_of(&s["hotkey"]);
        row(ui, "Hotkey", "Hold to talk (applied at next start)", |ui| {
            let label = if self.recording_hotkey {
                "Press the keys... (Esc cancels)".to_string()
            } else {
                format!("{hotkey}  (change)")
            };
            if ui.button(label).clicked() {
                self.recording_hotkey = !self.recording_hotkey;
                if self.recording_hotkey {
                    self.rec_pressed.clear();
                    self.rec_seen.clear();
                    if self.key_rx.is_none() {
                        let (tx, rx) = std::sync::mpsc::channel();
                        match crate::keysource::start(tx) {
                            Ok(_) => self.key_rx = Some(rx),
                            Err(e) => eprintln!("ascuta: key recorder: {e:#}"),
                        }
                    } else if let Some(rx) = &self.key_rx {
                        while rx.try_recv().is_ok() {} // drop stale events
                    }
                }
            }
        });
        // Presets for keyboards without a Windows key (Cmd/Option layouts):
        // "Win" is the Cmd key on Mac-style keyboards.
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Presets:").small().color(ON_SURFACE_VARIANT));
            for preset in ["Ctrl+Win", "Ctrl+Alt", "Ctrl+Shift", "RightAlt", "RightCtrl", "F8"] {
                let label = if preset == "Ctrl+Win" {
                    "Ctrl+Win / Ctrl+Cmd"
                } else {
                    preset
                };
                if text_button(ui, RichText::new(label).small().color(PRIMARY)).clicked() {
                    self.post(
                        "/api/settings",
                        json!({"hotkey": preset}),
                        "Hotkey saved. Press Restart to apply",
                    );
                }
            }
        });
        ui.label(
            RichText::new(
                "Win = the Windows/Super key; on Mac-style keyboards it is Cmd. \
                 Click the hotkey button and press the keys you want (modifier-only works).",
            )
            .small()
            .color(ON_SURFACE_VARIANT),
        );
        ui.separator();
        row(
            ui,
            "Restart Ascuta",
            "Applies hotkey, port and phone-bridge changes",
            |ui| {
                if primary_button(ui, "Restart").clicked() {
                    self.post("/api/restart", json!({}), "Restarting...");
                }
            },
        );
        for (key, title, sub) in [
            (
                "preserve_clipboard",
                "Restore clipboard",
                "Put back your clipboard after pasting",
            ),
            (
                "launch_at_login",
                "Launch at login",
                "Start Ascuta when you sign in",
            ),
        ] {
            let mut v = s[key].as_bool().unwrap_or(false);
            let before = v;
            row(ui, title, sub, |ui| {
                ui.checkbox(&mut v, "");
            });
            if v != before {
                self.post("/api/settings", json!({ key: v }), "Saved");
            }
        }

        section(ui, "Models");
        ui.label(
            RichText::new(format!("Stored in {}", str_of(&st["models_dir"])))
                .small()
                .color(ON_SURFACE_VARIANT),
        );
        ui.add_space(4.0);
        for m in st["models"].as_array().cloned().unwrap_or_default() {
            self.model_row(ui, &m, &model, true);
        }

        section(ui, "Transcript cleanup");
        ui.label(
            RichText::new(
                "Optional: a small local LLM (Gemma 4 E2B, same engine as the Android app) fixes punctuation and obvious \
                 mis-hearings after each dictation. Runs on the CPU; adds a few seconds.",
            )
            .small()
            .color(ON_SURFACE_VARIANT),
        );
        let available = st["cleanup_available"].as_bool().unwrap_or(false);
        let mut clean = s["cleanup_enabled"].as_bool().unwrap_or(false);
        let before = clean;
        row(
            ui,
            "Clean up transcripts",
            if available {
                "Applies punctuation and spelling fixes before typing"
            } else {
                "Not available in this build"
            },
            |ui| {
                ui.add_enabled(available, egui::Checkbox::new(&mut clean, ""));
            },
        );
        if clean != before {
            self.post("/api/settings", json!({"cleanup_enabled": clean}), "Saved");
        }
        if !st["cleanup_model"].is_null() {
            let cm = st["cleanup_model"].clone();
            self.model_row(ui, &cm, "", false);
        }

        section(ui, "Input from phone");
        ui.label(
            RichText::new(
                "The Ascuta Android app can type its transcripts on this computer, over your local network.",
            )
            .small()
            .color(ON_SURFACE_VARIANT),
        );
        let mut enabled = s["bridge_enabled"].as_bool().unwrap_or(false);
        let before = enabled;
        row(
            ui,
            "Send from phone",
            "Receive text from a paired phone (applied at next start)",
            |ui| {
                ui.checkbox(&mut enabled, "");
            },
        );
        if enabled != before {
            self.post(
                "/api/settings",
                json!({"bridge_enabled": enabled}),
                "Saved — restart Ascuta to apply",
            );
        }
        let server_port = s["bridge_port"].as_u64().unwrap_or(8737) as u16;
        let editing_port = ui.memory(|m| m.focused()).is_some();
        if !editing_port || self.port == 0 {
            self.port = server_port;
        }
        row(ui, "Port", "Your firewall must allow it", |ui| {
            let r = ui.add(egui::DragValue::new(&mut self.port).range(1024..=65535));
            if (r.drag_stopped() || r.lost_focus()) && self.port != server_port {
                self.post(
                    "/api/settings",
                    json!({"bridge_port": self.port}),
                    "Saved — restart Ascuta to apply",
                );
            }
        });
        if let Some(uri) = st["pairing"]["uri"].as_str() {
            ui.label(
                RichText::new(format!(
                    "Pair from the phone: Home > Send to computer > scan. ({})",
                    str_of(&st["pairing"]["name"])
                ))
                .small()
                .color(ON_SURFACE_VARIANT),
            );
            draw_qr(ui, uri);
        } else if enabled {
            ui.label(
                RichText::new("Restart Ascuta to show the pairing code.")
                    .small()
                    .color(ON_SURFACE_VARIANT),
            );
        }

        section(ui, "Files");
        row(ui, "Settings file", &str_of(&st["settings_path"]), |_| {});
    }
}

/// Open the settings window and block until it is closed.
pub fn run(base: String, token: String) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Ascuta — settings")
            .with_app_id("ascuta-settings")
            .with_inner_size([720.0, 860.0])
            .with_min_inner_size([340.0, 480.0]),
        ..Default::default()
    };
    let api = Api { base, token };
    eframe::run_native(
        "Ascuta",
        options,
        Box::new(move |cc| Ok(Box::new(SettingsApp::new(cc, api)))),
    )
    .map_err(|e| anyhow::anyhow!("settings window: {e}"))
}

impl SettingsApp {
    /// One model row: download / progress / use / delete. `selectable`
    /// models can be chosen as the speech model; others only (un)install.
    fn model_row(&mut self, ui: &mut egui::Ui, m: &Value, current: &str, selectable: bool) {
        let str_of = |v: &Value| v.as_str().unwrap_or("").to_string();
        let model = current.to_string();
        let id = str_of(&m["id"]);
            let desc = str_of(&m["description"]);
            let status = &m["status"];
            let state = status["state"].as_str().unwrap_or("missing");
            match state {
                "downloading" => {
                    let done = status["done"].as_u64().unwrap_or(0);
                    let total = status["total"].as_u64();
                    let frac = total.filter(|t| *t > 0).map(|t| done as f32 / t as f32);
                    ui.label(RichText::new(&id));
                    ui.add(
                        egui::ProgressBar::new(frac.unwrap_or(0.0))
                            .text(match total {
                                Some(t) => format!("{} / {} MB", mb(done), mb(t)),
                                None => format!("{} MB", mb(done)),
                            })
                            .animate(frac.is_none()),
                    );
                    ui.separator();
                }
                "ready" => {
                    let in_use = id == model;
                    row(ui, &id, &format!("{desc} — downloaded"), |ui| {
                        if text_button(ui, RichText::new("Delete").color(ERROR)).clicked() {
                            self.confirm_delete = Some(id.clone());
                        }
                        if selectable {
                            if in_use {
                                ui.label(RichText::new("In use").color(ON_SURFACE_VARIANT));
                            } else if text_button(ui, RichText::new("Use").color(PRIMARY))
                                .clicked()
                            {
                                self.post("/api/settings", json!({"model": id}), "Model changed");
                            }
                        }
                    });
                }
                "failed" => {
                    let err = str_of(&status["error"]);
                    row(ui, &id, &format!("Download failed: {err}"), |ui| {
                        if primary_button(ui, "Retry").clicked() {
                            self.post("/api/download", json!({"model": id}), "");
                        }
                    });
                }
                _ => {
                    row(ui, &id, &desc, |ui| {
                        if primary_button(ui, "Download").clicked() {
                            self.post("/api/download", json!({"model": id}), "");
                        }
                    });
                }
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_strings() {
        let ctrl = egui::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(hotkey_string(egui::Key::B, ctrl), "Ctrl+B");
        let both = egui::Modifiers {
            alt: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(hotkey_string(egui::Key::Space, both), "Alt+Shift+Space");
        assert_eq!(hotkey_string(egui::Key::F5, Default::default()), "F5");
    }
}
