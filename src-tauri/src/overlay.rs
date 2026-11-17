//! The "Flow bar": a small always-on-top pill at the bottom centre of the
//! screen, modelled on Wispr Flow's behaviour (docs.wisprflow.ai):
//!
//! The bar is hidden when idle — the tray icon shows the app is running.
//!
//! * **recording** — expands into a pill with a cancel (X) button, a live
//!   waveform and a stop (■) button; Esc also cancels;
//! * **processing** — same pill with a travelling highlight.
//!
//! Pure-Rust `winit` window (NOT a Tauri webview) painted with `softbuffer`.
//! The window has a fixed size chosen up front (some Wayland compositors
//! ignore resizes on mapped surfaces); the capsule is drawn inside it. On
//! Linux it is an X11/XWayland "notification" window so the window manager
//! never gives it keyboard focus (the paste target keeps it).

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Instant;

use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes, WindowLevel};

/// A clickable control on the expanded bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Cancel,
    Stop,
}

pub trait Overlay {
    fn show_listening(&mut self);
    /// "Transcribing…"
    fn show_decoding(&mut self);
    /// "Cleaning up…" (optional LLM pass after the transcript).
    fn show_cleaning(&mut self);
    /// Back to hidden (idle — the tray icon shows the app is running).
    fn hide(&mut self);
    /// Live input level, 0..1, while listening.
    fn set_level(&mut self, level: f32);
    /// Advance animations (called every loop iteration while visible).
    fn tick(&mut self) {}
    fn visible(&self) -> bool;
}

// Logical geometry. The window is the bounding box of the expanded pill.
pub const WIN_W: f32 = 260.0;
pub const WIN_H: f32 = 56.0;
const PILL_W: f32 = 220.0;
const PILL_H: f32 = 40.0;
const BTN_R: f32 = 13.0;
const BARS: usize = 20;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Hidden,
    Listening,
    Decoding,
    Cleaning,
}

/// The bar window: native Wayland layer surface when available (crisp on
/// scaled monitors), otherwise the X11/winit window.
pub enum Bar {
    X11(WinitOverlay),
    #[cfg(target_os = "linux")]
    Layer(crate::layer_bar::LayerBar),
}

// On platforms without the Wayland variant `Bar` has a single variant, so
// `if let Bar::X11(..)` is irrefutable there; that is intended.
#[allow(irrefutable_let_patterns)]
impl Bar {
    /// `on_button` is called from the Wayland thread on a button click.
    pub fn new(on_button: impl Fn(Button) + Send + 'static) -> Self {
        #[cfg(target_os = "linux")]
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            if let Some(b) = crate::layer_bar::LayerBar::start(Box::new(on_button)) {
                eprintln!("ascuta: Flow bar on a native Wayland layer surface");
                return Bar::Layer(b);
            }
            eprintln!("ascuta: no layer-shell; using the X11 window for the Flow bar");
        }
        #[cfg(not(target_os = "linux"))]
        let _ = on_button;
        Bar::X11(WinitOverlay::new())
    }

    pub fn ensure_window(&mut self, el: &ActiveEventLoop) {
        if let Bar::X11(o) = self {
            o.ensure_window(el);
        }
    }

    pub fn mode(&self) -> Mode {
        match self {
            Bar::X11(o) => o.mode(),
            #[cfg(target_os = "linux")]
            Bar::Layer(l) => l.mode(),
        }
    }

    pub fn scale_factor(&self) -> f64 {
        match self {
            Bar::X11(o) => o.scale_factor(),
            #[cfg(target_os = "linux")]
            Bar::Layer(_) => 1.0,
        }
    }

    pub fn redraw(&mut self) {
        if let Bar::X11(o) = self {
            o.redraw();
        }
    }
}

#[allow(irrefutable_let_patterns)]
impl Overlay for Bar {
    fn show_listening(&mut self) {
        match self {
            Bar::X11(o) => o.show_listening(),
            #[cfg(target_os = "linux")]
            Bar::Layer(l) => l.show(Mode::Listening),
        }
    }
    fn show_decoding(&mut self) {
        match self {
            Bar::X11(o) => o.show_decoding(),
            #[cfg(target_os = "linux")]
            Bar::Layer(l) => l.show(Mode::Decoding),
        }
    }
    fn show_cleaning(&mut self) {
        match self {
            Bar::X11(o) => o.show_cleaning(),
            #[cfg(target_os = "linux")]
            Bar::Layer(l) => l.show(Mode::Cleaning),
        }
    }
    fn hide(&mut self) {
        match self {
            Bar::X11(o) => o.hide(),
            #[cfg(target_os = "linux")]
            Bar::Layer(l) => l.show(Mode::Hidden),
        }
    }
    fn set_level(&mut self, level: f32) {
        match self {
            Bar::X11(o) => o.set_level(level),
            #[cfg(target_os = "linux")]
            Bar::Layer(l) => l.set_level(level),
        }
    }
    fn tick(&mut self) {
        if let Bar::X11(o) = self {
            o.tick();
        }
    }
    fn visible(&self) -> bool {
        self.mode() != Mode::Hidden
    }
}

/// Which control (if any) is under the logical point `(x, y)` of the window.
pub fn hit_test(mode: Mode, x: f32, y: f32) -> Option<Button> {
    if !matches!(mode, Mode::Listening) {
        return None;
    }
    let (cx, cy) = (WIN_W / 2.0, WIN_H / 2.0);
    let left = (cx - PILL_W / 2.0 + 8.0 + BTN_R, cy);
    let right = (cx + PILL_W / 2.0 - 8.0 - BTN_R, cy);
    let near = |p: (f32, f32)| ((x - p.0).powi(2) + (y - p.1).powi(2)).sqrt() <= BTN_R + 4.0;
    if near(left) {
        Some(Button::Cancel)
    } else if near(right) {
        Some(Button::Stop)
    } else {
        None
    }
}

/// Windows: make the bar a tool window that never takes activation, so the
/// paste target keeps keyboard focus.
#[cfg(windows)]
fn no_activate_windows(w: &Window) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Ok(h) = w.window_handle() {
        if let RawWindowHandle::Win32(h) = h.as_raw() {
            let hwnd = h.hwnd.get() as *mut std::ffi::c_void;
            // SAFETY: `hwnd` is the live handle of the window we just created.
            unsafe {
                let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                SetWindowLongPtrW(
                    hwnd,
                    GWL_EXSTYLE,
                    ex | (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) as isize,
                );
            }
        }
    }
}

pub struct WinitOverlay {
    window: Option<Arc<Window>>,
    context: Option<softbuffer::Context<Arc<Window>>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    mode: Mode,
    level: f32,
    started: Instant,
    font: Option<ab_glyph::FontVec>,
}

impl WinitOverlay {
    pub fn new() -> Self {
        Self {
            window: None,
            context: None,
            surface: None,
            mode: Mode::Hidden,
            level: 0.0,
            started: Instant::now(),
            font: crate::sysfont::bytes().and_then(|b| ab_glyph::FontVec::try_from_vec(b).ok()),
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn scale_factor(&self) -> f64 {
        self.window.as_ref().map_or(1.0, |w| w.scale_factor())
    }

    /// Create the bar on the event-loop thread (winit requirement).
    pub fn ensure_window(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attrs = WindowAttributes::default()
            .with_title("ascuta")
            .with_transparent(true)
            .with_decorations(false)
            .with_resizable(false)
            .with_visible(false)
            .with_active(false)
            .with_inner_size(LogicalSize::new(WIN_W as f64, WIN_H as f64))
            .with_window_level(WindowLevel::AlwaysOnTop);

        // Bottom-centre of the primary monitor, a little above the edge so
        // an auto-hiding dock/taskbar can still appear.
        if let Some(m) = event_loop.primary_monitor() {
            let scale = m.scale_factor();
            let (mw, mh) = (m.size().width as f64, m.size().height as f64);
            let (w, h) = (WIN_W as f64 * scale, WIN_H as f64 * scale);
            attrs = attrs.with_position(PhysicalPosition::new(
                m.position().x as f64 + (mw - w) / 2.0,
                m.position().y as f64 + mh - h - 40.0 * scale,
            ));
        }
        #[cfg(target_os = "linux")]
        {
            use winit::platform::x11::{WindowAttributesExtX11, WindowType};
            attrs = attrs.with_x11_window_type(vec![WindowType::Notification]);
        }
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs = attrs.with_has_shadow(false);
        }
        match event_loop.create_window(attrs) {
            Ok(w) => {
                let w = Arc::new(w);
                #[cfg(windows)]
                no_activate_windows(&w);
                // Clicks pass through except while the expanded bar with its
                // buttons is shown (see `set_mode`).
                let _ = w.set_cursor_hittest(false);
                match softbuffer::Context::new(w.clone()) {
                    Ok(ctx) => {
                        match softbuffer::Surface::new(&ctx, w.clone()) {
                            Ok(sf) => self.surface = Some(sf),
                            Err(e) => eprintln!("ascuta: bar surface failed: {e}"),
                        }
                        self.context = Some(ctx);
                    }
                    Err(e) => eprintln!("ascuta: overlay surface failed: {e}"),
                }
                self.window = Some(w);
                self.set_mode(Mode::Hidden);
            }
            Err(e) => eprintln!("ascuta: overlay window failed: {e}"),
        }
    }

    fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        if let Some(w) = &self.window {
            w.set_visible(mode != Mode::Hidden);
            let _ = w.set_cursor_hittest(mode == Mode::Listening);
        }
        if mode != Mode::Hidden {
            self.draw();
            // The window may not be mapped yet: also ask for an expose-time
            // repaint (handled in `redraw`) so the first frame is not lost.
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }

    /// Repaint on expose / `RedrawRequested`.
    pub fn redraw(&mut self) {
        if self.mode != Mode::Hidden {
            self.draw();
        }
    }

    fn draw(&mut self) {
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
            return;
        };
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        if let Err(e) = surface.resize(w, h) {
            eprintln!("ascuta: bar resize failed: {e}");
            return;
        }
        let mut buf = match surface.buffer_mut() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("ascuta: bar buffer failed: {e}");
                return;
            }
        };
        let (wp, hp) = (size.width as usize, size.height as usize);
        let t = self.started.elapsed().as_secs_f32();
        let label = match self.mode {
            Mode::Decoding => Some("Transcribing"),
            Mode::Cleaning => Some("Cleaning up"),
            _ => None,
        };
        let labelled = label.is_some() && self.font.is_some();
        render_bar(&mut buf, wp, hp, self.mode, self.level, t, labelled);
        if let (Some(text), Some(font)) = (label, &self.font) {
            // Animated ellipsis, fixed width so the text does not jitter.
            let dots = ".".repeat(1 + (t * 2.0) as usize % 3);
            draw_label(&mut buf, wp, hp, font, text, &dots);
        }
        if let Err(e) = buf.present() {
            eprintln!("ascuta: bar present failed: {e}");
        }
    }
}

/// Premultiplied ARGB.
fn argb(a: u8, r: u8, g: u8, b: u8) -> u32 {
    let m = |c: u8| c as u32 * a as u32 / 255;
    ((a as u32) << 24) | (m(r) << 16) | (m(g) << 8) | m(b)
}

/// Signed distance to a rounded rect centred at `(cx, cy)`; negative inside.
fn sd_round_rect(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let (dx, dy) = ((px - cx).abs() - (hw - r), (py - cy).abs() - (hh - r));
    (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() + dx.max(dy).min(0.0) - r
}

/// Distance from a point to the segment `a`–`b`.
fn sd_segment(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (pax, pay) = (px - a.0, py - a.1);
    let (bax, bay) = (b.0 - a.0, b.1 - a.1);
    let h = ((pax * bax + pay * bay) / (bax * bax + bay * bay)).clamp(0.0, 1.0);
    ((pax - bax * h).powi(2) + (pay - bay * h).powi(2)).sqrt()
}

/// Anti-aliased coverage from a signed distance given in *physical* pixels
/// (negative = inside): a crisp one-pixel edge, no halo.
fn cover(d_px: f32) -> f32 {
    (0.5 - d_px).clamp(0.0, 1.0)
}

type Rgb = (f32, f32, f32);

fn over(dst: Rgb, src: Rgb, a: f32) -> Rgb {
    (
        dst.0 + (src.0 - dst.0) * a,
        dst.1 + (src.1 - dst.1) * a,
        dst.2 + (src.2 - dst.2) * a,
    )
}

const BODY: Rgb = (0x12 as f32, 0x13 as f32, 0x17 as f32);
const BORDER: Rgb = (0x4A as f32, 0x4D as f32, 0x57 as f32);
const BUTTON: Rgb = (0x2C as f32, 0x2E as f32, 0x36 as f32);
const WHITE: Rgb = (255.0, 255.0, 255.0);

/// Paint the bar for `mode` into a `w × h` premultiplied-ARGB buffer.
/// Every shape is a signed-distance field with a one-pixel anti-aliased
/// edge, so it stays sharp at any scale. A pure function of its inputs
/// (unit-tested without a window).
pub(crate) fn render_bar(
    buf: &mut [u32],
    w: usize,
    h: usize,
    mode: Mode,
    level: f32,
    t: f32,
    labelled: bool,
) {
    buf.fill(0);
    if mode == Mode::Hidden {
        return;
    }
    let s = w as f32 / WIN_W; // physical px per logical px
    let (cx, cy) = (WIN_W / 2.0, WIN_H / 2.0);
    // With a text label the busy states show just the label (no X, ■ or
    // waveform), like the status line of the Android app.
    let busy_label = labelled && matches!(mode, Mode::Decoding | Mode::Cleaning);

    let amp = |i: usize| -> f32 {
        match mode {
            Mode::Listening => {
                let wob = 0.5 + 0.5 * (t * 8.0 + i as f32 * 0.9).sin().abs();
                0.12 + level.clamp(0.0, 1.0) * 0.88 * wob
            }
            Mode::Decoding | Mode::Cleaning => {
                let pos = (t * 1.5).fract() * (BARS as f32 + 6.0) - 3.0;
                0.12 + (1.0 - ((i as f32 - pos).abs() / 3.5)).clamp(0.0, 1.0) * 0.6
            }
            Mode::Hidden => 0.0,
        }
    };

    let wave_l = cx - PILL_W / 2.0 + 8.0 + BTN_R * 2.0 + 12.0;
    let wave_r = cx + PILL_W / 2.0 - 8.0 - BTN_R * 2.0 - 12.0;
    let slot = (wave_r - wave_l) / BARS as f32;
    let bar_hw = 1.5; // half width of a waveform bar, logical px
    let (lbx, rbx) = (
        cx - PILL_W / 2.0 + 8.0 + BTN_R,
        cx + PILL_W / 2.0 - 8.0 - BTN_R,
    );

    for y in 0..h {
        for x in 0..w {
            let (lx, ly) = ((x as f32 + 0.5) / s, (y as f32 + 0.5) / s);
            // Capsule body: opaque, with a crisp 1-physical-pixel border.
            let d = sd_round_rect(lx, ly, cx, cy, PILL_W / 2.0, PILL_H / 2.0, PILL_H / 2.0) * s;
            let body = cover(d);
            if body <= 0.0 {
                continue;
            }
            let inner = cover(d + 1.0); // 1 deep inside, 0 on the border ring
            let mut c = over(BORDER, BODY, inner);

            if !busy_label {
                // Cancel (X) and stop (■) buttons.
                for (bx, is_cancel) in [(lbx, true), (rbx, false)] {
                    let disc = cover(((lx - bx).hypot(ly - cy) - BTN_R) * s);
                    if disc > 0.0 {
                        c = over(c, BUTTON, disc);
                        let glyph = if is_cancel {
                            let k = 4.0;
                            let d1 = sd_segment(lx, ly, (bx - k, cy - k), (bx + k, cy + k));
                            let d2 = sd_segment(lx, ly, (bx - k, cy + k), (bx + k, cy - k));
                            (d1.min(d2) - 0.85) * s
                        } else if mode == Mode::Listening {
                            sd_round_rect(lx, ly, bx, cy, 4.0, 4.0, 1.2) * s
                        } else {
                            1.0
                        };
                        c = over(c, WHITE, cover(glyph) * disc);
                    }
                }
                // Waveform: rounded capsules.
                if lx >= wave_l - slot && lx < wave_r + slot {
                    let i = (((lx - wave_l) / slot).floor().max(0.0) as usize).min(BARS - 1);
                    let bx = wave_l + (i as f32 + 0.5) * slot;
                    let half = (PILL_H * 0.36 * amp(i)).max(bar_hw);
                    let bd = sd_round_rect(lx, ly, bx, cy, bar_hw, half, bar_hw) * s;
                    c = over(c, WHITE, cover(bd));
                }
            }
            buf[y * w + x] = argb((255.0 * body) as u8, c.0 as u8, c.1 as u8, c.2 as u8);
        }
    }
}

/// White text centred in the pill, composited over the premultiplied bar.
pub(crate) fn draw_label(
    buf: &mut [u32],
    w: usize,
    h: usize,
    font: &ab_glyph::FontVec,
    text: &str,
    dots: &str,
) {
    use ab_glyph::{point, Font, ScaleFont};
    let s = w as f32 / WIN_W;
    let size = 20.0 * s;
    let scaled = font.as_scaled(ab_glyph::PxScale::from(size));
    let advance = |t: &str| -> f32 {
        t.chars()
            .map(|c| scaled.h_advance(scaled.glyph_id(c)))
            .sum()
    };
    // Width is that of the text plus three dots, so it stays put.
    let total = advance(text) + advance("...");
    let mut x = (w as f32 - total) / 2.0;
    let baseline = h as f32 / 2.0 + (scaled.ascent() + scaled.descent()) / 2.0;
    for ch in text.chars().chain(dots.chars()) {
        let id = scaled.glyph_id(ch);
        let glyph = id.with_scale_and_position(ab_glyph::PxScale::from(size), point(x, baseline));
        x += scaled.h_advance(id);
        if let Some(og) = font.outline_glyph(glyph) {
            let b = og.px_bounds();
            og.draw(|gx, gy, cov| {
                let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                if px < 0 || py < 0 || px as usize >= w || py as usize >= h {
                    return;
                }
                let i = py as usize * w + px as usize;
                let dst = buf[i];
                if dst >> 24 == 0 {
                    return; // outside the capsule
                }
                let cov = cov.powf(0.75); // slightly heavier strokes
                let mix = |d: u32| (255.0 * cov + d as f32 * (1.0 - cov)) as u32;
                let (a, r, g, bl) = (
                    mix(dst >> 24),
                    mix((dst >> 16) & 0xFF),
                    mix((dst >> 8) & 0xFF),
                    mix(dst & 0xFF),
                );
                buf[i] = (a << 24) | (r << 16) | (g << 8) | bl;
            });
        }
    }
}

impl Overlay for WinitOverlay {
    fn show_listening(&mut self) {
        self.level = 0.0;
        self.set_mode(Mode::Listening);
    }
    fn show_decoding(&mut self) {
        self.set_mode(Mode::Decoding);
    }
    fn show_cleaning(&mut self) {
        self.set_mode(Mode::Cleaning);
    }
    fn hide(&mut self) {
        self.set_mode(Mode::Hidden);
    }
    fn set_level(&mut self, level: f32) {
        self.level = self.level * 0.5 + level.clamp(0.0, 1.0) * 0.5;
        if self.mode == Mode::Listening {
            self.draw();
        }
    }
    fn tick(&mut self) {
        if matches!(self.mode, Mode::Decoding | Mode::Cleaning) {
            self.draw();
        }
    }
    fn visible(&self) -> bool {
        self.mode != Mode::Hidden
    }
}

pub struct FakeOverlay {
    visible: bool,
    pub level: f32,
    pub log: Vec<&'static str>,
}

impl FakeOverlay {
    pub fn new() -> Self {
        Self {
            visible: false,
            level: 0.0,
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
    fn show_cleaning(&mut self) {
        self.visible = true;
        self.log.push("cleaning");
    }
    fn hide(&mut self) {
        self.visible = false;
        self.log.push("hide");
    }
    fn set_level(&mut self, level: f32) {
        self.level = level;
    }
    fn visible(&self) -> bool {
        self.visible
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn white(b: &[u32]) -> usize {
        b.iter()
            .filter(|p| (**p >> 24) > 0xC0 && (**p & 0xFF) > 0xE0 && ((**p >> 16) & 0xFF) > 0xE0)
            .count()
    }

    fn render(mode: Mode, level: f32) -> Vec<u32> {
        let (w, h) = (WIN_W as usize, WIN_H as usize);
        let mut buf = vec![0u32; w * h];
        render_bar(&mut buf, w, h, mode, level, 0.3, false);
        buf
    }

    /// `cargo test dump_bar -- --ignored` writes PPMs for visual review.
    #[test]
    #[ignore]
    fn dump_bar() {
        for (scale, tag) in [(1usize, "1x"), (2, "2x")] {
            let (w, h) = (WIN_W as usize * scale, WIN_H as usize * scale);
            let font =
                crate::sysfont::bytes().and_then(|b| ab_glyph::FontVec::try_from_vec(b).ok());
            for (name, mode, label) in [
                ("listening", Mode::Listening, None),
                ("transcribing", Mode::Decoding, Some("Transcribing")),
                ("cleaning", Mode::Cleaning, Some("Cleaning up")),
            ] {
                let mut buf = vec![0u32; w * h];
                render_bar(
                    &mut buf,
                    w,
                    h,
                    mode,
                    0.6,
                    0.35,
                    label.is_some() && font.is_some(),
                );
                if let (Some(text), Some(f)) = (label, &font) {
                    draw_label(&mut buf, w, h, f, text, "..");
                }
                // composite over a mid-grey "desktop"
                let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
                for p in &buf {
                    let a = (p >> 24) as f32 / 255.0;
                    for sh in [16, 8, 0] {
                        let c = ((p >> sh) & 0xFF) as f32;
                        out.push((c + 200.0 * (1.0 - a)) as u8);
                    }
                }
                let path = std::env::temp_dir().join(format!("ascuta-bar-{name}-{tag}.ppm"));
                std::fs::write(&path, out).unwrap();
                eprintln!("wrote {}", path.display());
            }
        }
    }

    #[test]
    fn labelled_busy_state_draws_text_not_buttons() {
        let Some(bytes) = crate::sysfont::bytes() else {
            return; // no system font on this machine
        };
        let font = ab_glyph::FontVec::try_from_vec(bytes).unwrap();
        let (w, h) = (WIN_W as usize, WIN_H as usize);
        let mut plain = vec![0u32; w * h];
        render_bar(&mut plain, w, h, Mode::Decoding, 0.0, 0.3, true);
        let mut text = plain.clone();
        draw_label(&mut text, w, h, &font, "Transcribing", "...");
        assert_ne!(plain, text, "label changed some pixels");
        // text pixels are clearly brighter than the dark capsule
        let bright = |b: &[u32]| b.iter().filter(|p| ((**p >> 16) & 0xFF) > 0x90).count();
        assert!(
            bright(&text) > bright(&plain) + 40,
            "{} vs {}",
            bright(&text),
            bright(&plain)
        );
        // outside the capsule stays transparent
        assert_eq!(text[0] >> 24, 0);
    }

    #[test]
    fn hidden_is_fully_transparent() {
        assert!(render(Mode::Hidden, 0.0).iter().all(|p| *p == 0));
    }

    #[test]
    fn listening_renders_visible_pill() {
        let opaque = |b: &[u32]| b.iter().filter(|p| (**p >> 24) > 0x80).count();
        let open = opaque(&render(Mode::Listening, 0.0));
        let decoding = opaque(&render(Mode::Decoding, 0.0));
        assert!(open > 400, "{open}");
        assert!(decoding > 400, "{decoding}");
    }

    #[test]
    fn louder_input_draws_taller_waveform() {
        let quiet = white(&render(Mode::Listening, 0.0));
        let loud = white(&render(Mode::Listening, 1.0));
        assert!(loud > quiet, "{loud} vs {quiet}");
    }

    #[test]
    fn buttons_hit_test() {
        let (cx, cy) = (WIN_W / 2.0, WIN_H / 2.0);
        let left = cx - PILL_W / 2.0 + 8.0 + BTN_R;
        let right = cx + PILL_W / 2.0 - 8.0 - BTN_R;
        assert_eq!(hit_test(Mode::Listening, left, cy), Some(Button::Cancel));
        assert_eq!(hit_test(Mode::Listening, right, cy), Some(Button::Stop));
        assert_eq!(hit_test(Mode::Listening, cx, cy), None);
        assert_eq!(hit_test(Mode::Hidden, left, cy), None);
        assert_eq!(hit_test(Mode::Decoding, left, cy), None);
    }

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
