//! System tray, one backend per platform (NOT the Tauri tray):
//! * Linux — StatusNotifier over D-Bus (`ksni`);
//! * macOS / Windows — the `tray-icon` crate (menu-bar item / notification
//!   area), created on the event-loop thread.
//!
//! PLAN §8/§11: Tauri tray icons go missing on Wayland GNOME (dev/.deb),
//! so the tray lives in Rust core. `muda` was evaluated first but needs
//! GTK/cairo system dev libs at build time; `ksni` is dependency-free and
//! speaks StatusNotifier natively (KDE; GNOME via its AppIndicator
//! extension). The `Tray` trait keeps the backend swappable.
//! `FakeTray` keeps CI headless (see `--check`).

#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::Result;
#[cfg(target_os = "linux")]
use ksni::blocking::{Handle, TrayMethods};

pub trait Tray {
    fn set_status(&self, status: &str);
}

/// The tray backend of this platform.
#[cfg(target_os = "linux")]
pub type SystemTray = KsniTray;
#[cfg(not(target_os = "linux"))]
pub type SystemTray = NativeTray;

#[cfg(target_os = "linux")]
struct AscutaTray {
    status: std::sync::Mutex<String>,
    on_quit: Box<dyn Fn() + Send + 'static>,
    on_settings: Box<dyn Fn() + Send + 'static>,
}

#[cfg(target_os = "linux")]
impl ksni::Tray for AscutaTray {
    fn id(&self) -> String {
        "ascuta".into()
    }
    fn title(&self) -> String {
        "ascuta — dictation".into()
    }
    // Freedesktop icon name — no asset files needed for M0.
    fn icon_name(&self) -> String {
        if self.status.lock().unwrap().as_str() == "idle" {
            "microphone-sensitivity-muted".into()
        } else {
            "microphone-sensitivity-high".into()
        }
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            icon_name: Default::default(),
            icon_pixmap: Default::default(),
            title: "ascuta".into(),
            description: format!("dictation — {}", self.status.lock().unwrap()),
        }
    }
    /// Left click on the icon opens the settings window too.
    fn activate(&mut self, _x: i32, _y: i32) {
        (self.on_settings)();
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        vec![
            StandardItem {
                label: format!("ascuta: {}", self.status.lock().unwrap()),
                enabled: false,
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Show Settings".into(),
                activate: Box::new(|tray: &mut Self| (tray.on_settings)()),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|tray: &mut Self| (tray.on_quit)()),
                ..Default::default()
            }
            .into(),
        ]
    }
}

#[cfg(target_os = "linux")]
pub struct KsniTray {
    handle: Handle<AscutaTray>,
}

#[cfg(target_os = "linux")]
impl KsniTray {
    /// `on_quit` / `on_settings` run when the user picks them in the menu.
    pub fn new(
        on_quit: impl Fn() + Send + 'static,
        on_settings: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        let tray = AscutaTray {
            status: std::sync::Mutex::new("idle".to_string()),
            on_quit: Box::new(on_quit),
            on_settings: Box::new(on_settings),
        };
        let handle = tray.spawn().context("spawn StatusNotifier tray")?;
        Ok(Self { handle })
    }
}

#[cfg(target_os = "linux")]
impl Tray for KsniTray {
    fn set_status(&self, status: &str) {
        self.handle.update(|tray: &mut AscutaTray| {
            *tray.status.lock().unwrap() = status.to_string();
        });
    }
}

/// 32×32 RGBA tray icon: a coloured disc with a white microphone. Pure, so
/// it is unit-tested; the colour encodes the state.
pub fn icon_rgba(status: &str) -> (Vec<u8>, u32, u32) {
    const N: u32 = 32;
    let (r, g, b) = match status {
        "recording" => (0xE5, 0x48, 0x4D),
        "decoding" | "cleaning" => (0xF5, 0xA5, 0x24),
        _ => (0x05, 0x82, 0xCA),
    };
    let cover = |d: f32| (0.5 - d).clamp(0.0, 1.0);
    let seg = |px: f32, py: f32, a: (f32, f32), b: (f32, f32)| -> f32 {
        let (pax, pay) = (px - a.0, py - a.1);
        let (bax, bay) = (b.0 - a.0, b.1 - a.1);
        let h = ((pax * bax + pay * bay) / (bax * bax + bay * bay)).clamp(0.0, 1.0);
        ((pax - bax * h).powi(2) + (pay - bay * h).powi(2)).sqrt()
    };
    let mut out = vec![0u8; (N * N * 4) as usize];
    for y in 0..N {
        for x in 0..N {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let disc = cover((px - 16.0).hypot(py - 16.0) - 15.0);
            if disc <= 0.0 {
                continue;
            }
            // microphone: capsule, cradle arc, stem and base
            let body = cover(seg(px, py, (16.0, 10.5), (16.0, 15.5)) - 3.6);
            let cradle = {
                let d = ((px - 16.0).hypot(py - 15.5) - 7.2).abs() - 0.9;
                if py >= 15.5 {
                    cover(d)
                } else {
                    0.0
                }
            };
            let stem = cover(seg(px, py, (16.0, 22.5), (16.0, 25.0)) - 0.9);
            let base = cover(seg(px, py, (12.0, 25.5), (20.0, 25.5)) - 0.9);
            let glyph = body.max(cradle).max(stem).max(base);
            let mix = |c: u8| (c as f32 + (255.0 - c as f32) * glyph) as u8;
            let i = ((y * N + x) * 4) as usize;
            out[i] = mix(r);
            out[i + 1] = mix(g);
            out[i + 2] = mix(b);
            out[i + 3] = (255.0 * disc) as u8;
        }
    }
    (out, N, N)
}

/// macOS / Windows tray (`tray-icon`). Must be created on the event-loop
/// thread (after the loop started); menu clicks are polled from it.
#[cfg(not(target_os = "linux"))]
pub struct NativeTray {
    icon: tray_icon::TrayIcon,
    show_id: tray_icon::menu::MenuId,
    quit_id: tray_icon::menu::MenuId,
}

#[cfg(not(target_os = "linux"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ShowSettings,
    Quit,
}

#[cfg(not(target_os = "linux"))]
impl NativeTray {
    pub fn new() -> Result<Self> {
        use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
        let menu = Menu::new();
        let show = MenuItem::new("Show Settings", true, None);
        let quit = MenuItem::new("Quit", true, None);
        menu.append_items(&[&show, &PredefinedMenuItem::separator(), &quit])?;
        let (rgba, w, h) = icon_rgba("idle");
        let icon = tray_icon::TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("Ascuta")
            .with_icon(tray_icon::Icon::from_rgba(rgba, w, h)?)
            .build()?;
        Ok(Self {
            icon,
            show_id: show.id().clone(),
            quit_id: quit.id().clone(),
        })
    }

    /// Pending menu clicks (and left clicks on the icon, which open settings).
    pub fn poll(&self) -> Vec<TrayAction> {
        use tray_icon::menu::MenuEvent;
        use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
        let mut out = Vec::new();
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if ev.id == self.show_id {
                out.push(TrayAction::ShowSettings);
            } else if ev.id == self.quit_id {
                out.push(TrayAction::Quit);
            }
        }
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                out.push(TrayAction::ShowSettings);
            }
        }
        out
    }
}

#[cfg(not(target_os = "linux"))]
impl Tray for NativeTray {
    fn set_status(&self, status: &str) {
        let (rgba, w, h) = icon_rgba(status);
        if let Ok(icon) = tray_icon::Icon::from_rgba(rgba, w, h) {
            let _ = self.icon.set_icon(Some(icon));
        }
        let _ = self.icon.set_tooltip(Some(format!("Ascuta — {status}")));
    }
}

pub struct FakeTray {
    pub last_status: std::cell::RefCell<String>,
}

impl FakeTray {
    pub fn new() -> Self {
        Self {
            last_status: std::cell::RefCell::new("idle".to_string()),
        }
    }
}

impl Tray for FakeTray {
    fn set_status(&self, status: &str) {
        *self.last_status.borrow_mut() = status.to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_a_disc_and_white_glyph() {
        let (px, w, h) = icon_rgba("recording");
        assert_eq!((w, h, px.len()), (32, 32, 32 * 32 * 4));
        let at = |x: usize, y: usize| &px[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
        assert_eq!(at(0, 0)[3], 0, "corner transparent");
        assert_eq!(at(27, 16)[..3], [0xE5, 0x48, 0x4D], "disc colour");
        assert_eq!(at(16, 13)[..3], [255, 255, 255], "microphone body");
        assert_ne!(icon_rgba("idle").0, icon_rgba("recording").0);
    }

    #[test]
    fn fake_records_status() {
        let t = FakeTray::new();
        t.set_status("recording");
        assert_eq!(*t.last_status.borrow(), "recording");
    }
}
