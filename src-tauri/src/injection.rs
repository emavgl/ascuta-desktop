//! Text injection: clipboard paste → keystroke fallback.
//!
//! M0 pastes the stub transcript (`hello`) at the cursor:
//! 1. Optionally preserve the user's clipboard (default on).
//! 2. Set clipboard to the text, emit Ctrl+V via `enigo`.
//! 3. On failure (e.g. Wayland X11-only session — PLAN §11 documents
//!    X11-first, clipboard + manual paste fallback) the text stays in the
//!    clipboard and the error bubbles up instead of losing the dictation.
//! 4. Restore the previous clipboard.
//!
//! `Injector` trait + `FakeInjector` keep headless CI honest.

use anyhow::{Context, Result};
use enigo::{Direction, Enigo, Key, Keyboard, Settings as EnigoSettings};

/// Non-blocking desktop notice (never steals keyboard focus).
pub fn show_notice(title: &str, message: &str) {
    use std::process::Command;
    let tries: [(&str, Vec<String>); 3] = [
        (
            "kdialog",
            vec![
                "--title".into(),
                title.into(),
                "--passivepopup".into(),
                message.into(),
                "6".into(),
            ],
        ),
        (
            "notify-send",
            vec!["-a".into(), "Ascuta".into(), title.into(), message.into()],
        ),
        (
            "zenity",
            vec![
                "--notification".into(),
                format!("--text={title}: {message}"),
            ],
        ),
    ];
    for (bin, args) in tries {
        if Command::new(bin).args(&args).spawn().is_ok() {
            return;
        }
    }
    eprintln!("ascuta: {title}: {message}");
}

pub trait Injector {
    /// Put `text` in the clipboard without pasting (no text cursor).
    fn copy_only(&mut self, text: &str) -> Result<()>;

    fn paste_text(&mut self, text: &str, preserve_clipboard: bool) -> Result<()>;

    /// Type `text` as synthetic key events instead of pasting it.
    fn type_text(&mut self, text: &str) -> Result<()>;

    /// Send a Return keypress (phone bridge "submit after transcript").
    fn press_enter(&mut self) -> Result<()>;
}

/// Transcript as it should be inserted: trimmed, with a single trailing
/// space unless it already ends with whitespace. Consecutive dictations
/// (local or from the phone) then do not run together.
pub fn with_trailing_space(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        return String::new();
    }
    format!("{text} ")
}

pub struct EnigoInjector {
    clipboard: Option<arboard::Clipboard>,
    enigo: Option<Enigo>,
    /// Wayland: XDG RemoteDesktop portal, connected in the background at
    /// startup (the consent dialog appears once, then a token is stored).
    #[cfg(target_os = "linux")]
    portal: std::sync::Arc<std::sync::Mutex<Option<crate::portal_keys::PortalKeyboard>>>,
}

#[cfg(target_os = "linux")]
fn is_wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v == "wayland")
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// KDE/Wayland: set the clipboard through Klipper (works without a focused
/// window or the data-control protocol).
#[cfg(target_os = "linux")]
fn set_clipboard_klipper(text: &str) -> bool {
    std::process::Command::new("busctl")
        .args([
            "--user",
            "call",
            "org.kde.klipper",
            "/klipper",
            "org.kde.klipper.klipper",
            "setClipboardContents",
            "s",
            text,
        ])
        .output()
        .is_ok_and(|o| o.status.success())
}

impl EnigoInjector {
    pub fn new() -> Self {
        // Headless/odd sessions: backends may fail to init. Degrade to
        // clipboard-only rather than crashing the daemon.
        let clipboard = arboard::Clipboard::new()
            .map_err(|e| eprintln!("ascuta: clipboard unavailable: {e}"))
            .ok();
        let enigo = Enigo::new(&EnigoSettings::default())
            .map_err(|e| eprintln!("ascuta: key backend unavailable: {e}"))
            .ok();
        #[cfg(target_os = "linux")]
        let portal = std::sync::Arc::new(std::sync::Mutex::new(None));
        #[cfg(target_os = "linux")]
        if is_wayland() {
            let slot = portal.clone();
            std::thread::spawn(move || match crate::portal_keys::PortalKeyboard::connect() {
                Ok(p) => {
                    eprintln!("ascuta: keyboard portal ready (automatic paste enabled)");
                    *slot.lock().unwrap() = Some(p);
                }
                Err(e) => eprintln!("ascuta: {e:#} — text will be left in the clipboard"),
            });
        }
        Self {
            clipboard,
            enigo,
            #[cfg(target_os = "linux")]
            portal,
        }
    }

    fn type_fallback(&mut self, text: &str) -> Result<()> {
        let enigo = self
            .enigo
            .as_mut()
            .context("no key backend for typing")?;
        enigo.text(text).context("type text")?;
        Ok(())
    }
}

impl EnigoInjector {
    /// Set the clipboard (Klipper on KDE/Wayland, arboard otherwise).
    fn set_clipboard(&mut self, text: &str) -> Result<()> {
        #[cfg(target_os = "linux")]
        if is_wayland() && set_clipboard_klipper(text) {
            return Ok(());
        }
        let clip = self.clipboard.as_mut().context("no clipboard backend")?;
        clip.set_text(text.to_string()).context("set clipboard")
    }

    /// Send Ctrl+V to the focused window. Returns whether a key backend
    /// accepted it.
    fn send_paste(&mut self) -> bool {
        #[cfg(target_os = "linux")]
        if is_wayland() {
            if let Some(p) = self.portal.lock().unwrap().as_ref() {
                match p.paste() {
                    Ok(()) => return true,
                    Err(e) => eprintln!("ascuta: portal paste failed: {e:#}"),
                }
            }
            // Native Wayland windows ignore X11 XTEST; XWayland ones accept it.
        }
        match self.enigo.as_mut() {
            Some(enigo) => {
                // Paste is Cmd+V on macOS. On Linux/Windows use Shift+Insert:
                // it works in terminals (Ctrl+V does not) and in normal apps.
                #[cfg(target_os = "macos")]
                let (modifier, key) = (Key::Meta, Key::Unicode('v'));
                #[cfg(not(target_os = "macos"))]
                let (modifier, key) = (Key::Shift, Key::Insert);
                let r = (|| -> Result<()> {
                    enigo.key(modifier, Direction::Press)?;
                    enigo.key(key, Direction::Click)?;
                    enigo.key(modifier, Direction::Release)?;
                    Ok(())
                })();
                match r {
                    Ok(()) => true,
                    Err(e) => {
                        eprintln!("ascuta: ctrl+v failed ({e:#})");
                        false
                    }
                }
            }
            None => false,
        }
    }
}

impl Injector for EnigoInjector {
    fn copy_only(&mut self, text: &str) -> Result<()> {
        self.set_clipboard(text)
    }

    fn type_text(&mut self, text: &str) -> Result<()> {
        // Native Wayland windows only accept synthetic keys through the
        // portal; XTEST (enigo) would silently miss them. Without a portal,
        // report an error so the caller can fall back to pasting.
        #[cfg(target_os = "linux")]
        if is_wayland() {
            let portal = self.portal.lock().unwrap();
            return match portal.as_ref() {
                Some(p) => p.type_text(text),
                None => anyhow::bail!("Wayland keyboard portal not available"),
            };
        }
        self.type_fallback(text)
    }

    fn paste_text(&mut self, text: &str, preserve_clipboard: bool) -> Result<()> {
        let saved = if preserve_clipboard {
            self.clipboard.as_mut().and_then(|c| c.get_text().ok())
        } else {
            None
        };

        #[cfg(target_os = "linux")]
        let mut set = is_wayland() && set_clipboard_klipper(text);
        #[cfg(not(target_os = "linux"))]
        let mut set = false;
        if !set {
            if let Some(clip) = self.clipboard.as_mut() {
                clip.set_text(text.to_string()).context("set clipboard")?;
                set = true;
            }
        }
        if !set {
            anyhow::bail!("no clipboard backend");
        }

        let pasted = self.send_paste();
        if !pasted {
            // Keep the transcript in the clipboard so the user can paste it.
            eprintln!("ascuta: could not paste automatically; text is in the clipboard (Ctrl+V)");
            return Ok(());
        }

        if preserve_clipboard {
            if let Some(prev) = saved {
                // Give the target app time to read the clipboard first.
                std::thread::sleep(std::time::Duration::from_millis(300));
                #[cfg(target_os = "linux")]
                let restored = is_wayland() && set_clipboard_klipper(&prev);
                #[cfg(not(target_os = "linux"))]
                let restored = false;
                if !restored {
                    if let Some(clip) = self.clipboard.as_mut() {
                        let _ = clip.set_text(prev);
                    }
                }
            }
        }
        Ok(())
    }

    fn press_enter(&mut self) -> Result<()> {
        #[cfg(target_os = "linux")]
        if let Some(p) = self.portal.lock().unwrap().as_ref() {
            return p.enter();
        }
        let enigo = self.enigo.as_mut().context("no key backend for enter")?;
        enigo
            .key(Key::Return, Direction::Click)
            .context("press Enter")?;
        Ok(())
    }
}

pub struct FakeInjector {
    pasted: Vec<String>,
    typed: Vec<String>,
    copied: Vec<String>,
    enters: usize,
}

impl FakeInjector {
    pub fn new() -> Self {
        Self {
            pasted: Vec::new(),
            typed: Vec::new(),
            copied: Vec::new(),
            enters: 0,
        }
    }

    pub fn copied(&self) -> &[String] {
        &self.copied
    }

    pub fn pasted(&self) -> &[String] {
        &self.pasted
    }

    pub fn typed(&self) -> &[String] {
        &self.typed
    }

    pub fn enters(&self) -> usize {
        self.enters
    }
}

impl Injector for FakeInjector {
    fn copy_only(&mut self, text: &str) -> Result<()> {
        self.copied.push(text.to_string());
        Ok(())
    }

    fn paste_text(&mut self, text: &str, _preserve_clipboard: bool) -> Result<()> {
        self.pasted.push(text.to_string());
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<()> {
        self.typed.push(text.to_string());
        Ok(())
    }

    fn press_enter(&mut self) -> Result<()> {
        self.enters += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_records_paste() {
        let mut inj = FakeInjector::new();
        inj.paste_text("hello", true).unwrap();
        assert_eq!(inj.pasted(), &["hello".to_string()]);
    }

    #[test]
    fn fake_records_typing() {
        let mut inj = FakeInjector::new();
        inj.type_text("hello").unwrap();
        assert_eq!(inj.typed(), &["hello".to_string()]);
    }

    #[test]
    fn trailing_space_joins_dictations() {
        assert_eq!(with_trailing_space("hello"), "hello ");
        assert_eq!(with_trailing_space("  hello.\n"), "hello. ");
        assert_eq!(with_trailing_space(""), "");
        assert_eq!(with_trailing_space("   "), "");
    }

    #[test]
    fn fake_records_enter() {
        let mut inj = FakeInjector::new();
        inj.press_enter().unwrap();
        assert_eq!(inj.enters(), 1);
    }
}
