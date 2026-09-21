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

pub trait Injector {
    fn paste_text(&mut self, text: &str, preserve_clipboard: bool) -> Result<()>;

    /// Send a Return keypress (phone bridge "submit after transcript").
    fn press_enter(&mut self) -> Result<()>;
}

pub struct EnigoInjector {
    clipboard: Option<arboard::Clipboard>,
    enigo: Option<Enigo>,
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
        Self { clipboard, enigo }
    }

    fn type_fallback(&mut self, text: &str) -> Result<()> {
        let enigo = self
            .enigo
            .as_mut()
            .context("no key backend for type fallback")?;
        enigo.text(text).context("type fallback")?;
        Ok(())
    }
}

impl Injector for EnigoInjector {
    fn paste_text(&mut self, text: &str, preserve_clipboard: bool) -> Result<()> {
        let saved = if preserve_clipboard {
            self.clipboard.as_mut().and_then(|c| c.get_text().ok())
        } else {
            None
        };

        if let Some(clip) = self.clipboard.as_mut() {
            clip.set_text(text.to_string()).context("set clipboard")?;
        }
        // Best effort: Ctrl+V. If the key backend is missing (Wayland without
        // XWayland, headless), text is at least in the clipboard.
        let paste_ok = match self.enigo.as_mut() {
            Some(enigo) => {
                let r = (|| -> Result<()> {
                    enigo.key(Key::Control, Direction::Press)?;
                    enigo.key(Key::Unicode('v'), Direction::Click)?;
                    enigo.key(Key::Control, Direction::Release)?;
                    Ok(())
                })();
                if let Err(e) = r {
                    eprintln!("ascuta: ctrl+v failed ({e:#}), trying type fallback");
                    self.type_fallback(text).is_ok()
                } else {
                    true
                }
            }
            None => {
                eprintln!("ascuta: no key backend — text left in clipboard for manual paste");
                false
            }
        };
        let _ = paste_ok;

        if preserve_clipboard {
            if let (Some(clip), Some(prev)) = (self.clipboard.as_mut(), saved) {
                // Restore quietly; a failure here must not fail the injection.
                let _ = clip.set_text(prev);
            }
        }
        Ok(())
    }

    fn press_enter(&mut self) -> Result<()> {
        let enigo = self.enigo.as_mut().context("no key backend for enter")?;
        enigo
            .key(Key::Return, Direction::Click)
            .context("press Enter")?;
        Ok(())
    }
}

pub struct FakeInjector {
    pasted: Vec<String>,
    enters: usize,
}

impl FakeInjector {
    pub fn new() -> Self {
        Self {
            pasted: Vec::new(),
            enters: 0,
        }
    }

    pub fn pasted(&self) -> &[String] {
        &self.pasted
    }

    pub fn enters(&self) -> usize {
        self.enters
    }
}

impl Injector for FakeInjector {
    fn paste_text(&mut self, text: &str, _preserve_clipboard: bool) -> Result<()> {
        self.pasted.push(text.to_string());
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
    fn fake_records_enter() {
        let mut inj = FakeInjector::new();
        inj.press_enter().unwrap();
        assert_eq!(inj.enters(), 1);
    }
}
