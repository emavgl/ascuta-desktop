//! Global hotkey: native `global-hotkey` crate (X11 grab on Linux).
//!
//! Wayland fast-follow (PLAN §8/§11): route through the XDG
//! `GlobalShortcuts` portal via a single `wl_register_all()`-style call
//! with a stable portal ID + app ID (user confirms once in the compositor
//! dialog). `tauri-plugin-global-shortcut` is NOT used — its callback is
//! silent on Wayland GNOME. Until the portal path lands, Wayland sessions
//! should run the X11 backend (`WAYLAND_DISPLAY=` unset) where available.
//!
//! `FakeHotkey` feeds synthetic press/release for `--check`/unit tests.

use std::str::FromStr;
use std::sync::mpsc;

use anyhow::{Context, Result};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::settings::Settings;

pub struct HotkeyManager {
    _manager: GlobalHotKeyManager,
    _hotkey: HotKey,
    id: u32,
}

impl HotkeyManager {
    /// Register the user's hold-to-talk hotkey (default Ctrl+B).
    pub fn register_default(settings: &Settings) -> Result<Self> {
        let manager = GlobalHotKeyManager::new().context("hotkey manager")?;
        let hotkey = parse_hotkey(&settings.hotkey)?;
        let id = hotkey.id();
        manager.register(hotkey).context("register global hotkey")?;
        eprintln!("ascuta: hotkey registered: {}", settings.hotkey);
        Ok(Self {
            _manager: manager,
            _hotkey: hotkey,
            id,
        })
    }

    pub fn id(&self) -> u32 {
        self.id
    }
}

fn parse_hotkey(spec: &str) -> Result<HotKey> {
    // Accept `Ctrl+B` / `ctrl+b` / `Alt+Space`, plus a couple of aliases
    // the `global-hotkey` parser is strict about.
    let normalized = spec
        .trim()
        .replace("Ctrl+", "ctrl+")
        .replace("CTRL+", "ctrl+");
    if normalized.eq_ignore_ascii_case("fn") || normalized.eq_ignore_ascii_case("option+space") {
        // macOS-only defaults from PLAN §7; fall back to a safe combo here.
        // (Real mac hold-Fn support lands with the macOS build.)
        return Ok(HotKey::new(Some(Modifiers::ALT), Code::Space));
    }
    HotKey::from_str(&normalized).with_context(|| format!("parse hotkey {spec:?}"))
}

/// Forward press/release events for our hotkey id into the winit loop.
/// Runs on its own thread; never blocks inference (there is none in M0).
pub fn pump_events(tx: mpsc::Sender<super::OsEvent>) {
    for ev in GlobalHotKeyEvent::receiver().iter() {
        let msg = match ev.state {
            HotKeyState::Pressed => super::OsEvent::HotkeyPressed,
            HotKeyState::Released => super::OsEvent::HotkeyReleased,
        };
        if tx.send(msg).is_err() {
            break;
        }
    }
}

/// Synthetic press/release source for headless tests.
pub struct FakeHotkey {
    pub events: Vec<bool>, // true = pressed, false = released
}

impl FakeHotkey {
    pub fn press_release() -> Self {
        Self {
            events: vec![true, false],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_hotkey() {
        let hk = parse_hotkey("Ctrl+B").unwrap();
        assert_eq!(hk, HotKey::new(Some(Modifiers::CONTROL), Code::KeyB));
    }

    #[test]
    fn bad_hotkey_errors() {
        assert!(parse_hotkey("Not+A+Key+Xyz").is_err());
    }
}
