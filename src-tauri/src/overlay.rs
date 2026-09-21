//! HUD pill: pure-Rust `winit` window — NOT a Tauri webview.
//!
//! PLAN §8: Tauri has no per-pixel click-through and `alwaysOnTop` is not
//! honoured on Wayland, so the overlay is a small native window:
//! transparent, undecorated, click-through (`set_cursor_hittest(false)`),
//! fixed max size up front (some Wayland compositors ignore resizes on
//! mapped layer surfaces; layer-shell wiring is the Wayland fast-follow).
//! M0 shows state via the title bar text; the level meter lands in M1.

use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes, WindowLevel};

pub trait Overlay {
    fn show_listening(&mut self);
    fn show_decoding(&mut self);
    fn hide(&mut self);
    fn visible(&self) -> bool;
}

pub struct WinitOverlay {
    window: Option<Window>,
    visible: bool,
}

impl WinitOverlay {
    pub fn new() -> Self {
        Self {
            window: None,
            visible: false,
        }
    }

    /// Create the pill on the event-loop thread (winit requirement).
    pub fn ensure_window(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = WindowAttributes::default()
            .with_title("ascuta")
            .with_transparent(true)
            .with_decorations(false)
            .with_visible(false)
            // Fixed footprint up front — see module docs.
            .with_inner_size(winit::dpi::LogicalSize::new(280.0, 48.0))
            .with_window_level(WindowLevel::AlwaysOnTop);
        match event_loop.create_window(attrs) {
            Ok(w) => {
                // Non-interactive pill: clicks pass through to the app below.
                let _ = w.set_cursor_hittest(false);
                self.window = Some(w);
            }
            Err(e) => eprintln!("ascuta: overlay window failed: {e}"),
        }
    }

    fn set(&mut self, text: &str, show: bool) {
        self.visible = show;
        if let Some(w) = &self.window {
            w.set_title(text);
            w.set_visible(show);
            if show {
                w.request_redraw();
            }
        }
    }
}

impl Overlay for WinitOverlay {
    fn show_listening(&mut self) {
        self.set("ascuta — listening…", true);
    }
    fn show_decoding(&mut self) {
        self.set("ascuta — decoding…", true);
    }
    fn hide(&mut self) {
        self.set("ascuta", false);
    }
    fn visible(&self) -> bool {
        self.visible
    }
}

pub struct FakeOverlay {
    visible: bool,
    pub log: Vec<&'static str>,
}

impl FakeOverlay {
    pub fn new() -> Self {
        Self {
            visible: false,
            log: Vec::new(),
        }
    }
}

impl Overlay for FakeOverlay {
    fn show_listening(&mut self) {
        self.visible = true;
        self.log.push("listening");
    }
    fn show_decoding(&mut self) {
        self.visible = true;
        self.log.push("decoding");
    }
    fn hide(&mut self) {
        self.visible = false;
        self.log.push("hide");
    }
    fn visible(&self) -> bool {
        self.visible
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_overlay_tracks_visibility() {
        let mut o = FakeOverlay::new();
        o.show_listening();
        assert!(o.visible());
        o.hide();
        assert!(!o.visible());
        assert_eq!(o.log, vec!["listening", "hide"]);
    }
}
