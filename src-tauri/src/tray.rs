//! System tray: native StatusNotifier (`ksni`, pure-Rust D-Bus) — NOT Tauri tray.
//!
//! PLAN §8/§11: Tauri tray icons go missing on Wayland GNOME (dev/.deb),
//! so the tray lives in Rust core. `muda` was evaluated first but needs
//! GTK/cairo system dev libs at build time; `ksni` is dependency-free and
//! speaks StatusNotifier natively (KDE; GNOME via its AppIndicator
//! extension). The `Tray` trait keeps the backend swappable.
//! `FakeTray` keeps CI headless (see `--check`).

use anyhow::{Context, Result};
use ksni::blocking::{Handle, TrayMethods};

pub trait Tray {
    fn set_status(&self, status: &str);
}

struct AscutaTray {
    status: std::sync::Mutex<String>,
    on_quit: Box<dyn Fn() + Send + 'static>,
}

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
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        vec![
            StandardItem {
                label: format!("ascuta: {}", self.status.lock().unwrap()),
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit ascuta".into(),
                activate: Box::new(|tray: &mut Self| (tray.on_quit)()),
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub struct KsniTray {
    handle: Handle<AscutaTray>,
}

impl KsniTray {
    /// `on_quit` runs when the user picks Quit from the tray menu.
    pub fn new(on_quit: impl Fn() + Send + 'static) -> Result<Self> {
        let tray = AscutaTray {
            status: std::sync::Mutex::new("idle".to_string()),
            on_quit: Box::new(on_quit),
        };
        let handle = tray.spawn().context("spawn StatusNotifier tray")?;
        Ok(Self { handle })
    }
}

impl Tray for KsniTray {
    fn set_status(&self, status: &str) {
        self.handle.update(|tray: &mut AscutaTray| {
            *tray.status.lock().unwrap() = status.to_string();
        });
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
    fn fake_records_status() {
        let t = FakeTray::new();
        t.set_status("recording");
        assert_eq!(*t.last_status.borrow(), "recording");
    }
}
