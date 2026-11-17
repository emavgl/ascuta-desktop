//! The Flow bar as a **native Wayland layer-shell surface** (wlr-layer-shell,
//! supported by KWin, Sway, Hyprland, …).
//!
//! Why not a normal window: an X11 window under XWayland is a bitmap that
//! the compositor stretches on scaled monitors (e.g. 160%), which looks soft
//! and grainy. A layer surface is drawn by us at the output's real scale
//! (`wl_surface.set_buffer_scale`), never takes keyboard focus (so the paste
//! target keeps it) and is always on top. Falls back to the X11 window when
//! the compositor has no layer shell.
//!
//! The surface lives on its own thread with its own Wayland connection;
//! the main loop talks to it over a channel.

use std::os::fd::AsRawFd;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, Region};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::slot::SlotPool;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm, registry_handlers,
};
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface};
use wayland_client::{Connection, QueueHandle};

use crate::overlay::{draw_label, hit_test, render_bar, Button, Mode, WIN_H, WIN_W};

enum Cmd {
    Mode(Mode),
    Level(f32),
}

/// Handle used by the main thread.
pub struct LayerBar {
    tx: Sender<Cmd>,
    mode: Mode,
}

/// Events from clicks on the bar's buttons.
pub type ButtonSink = Box<dyn Fn(Button) + Send + 'static>;

impl LayerBar {
    /// Connect to the compositor and map the (initially invisible) bar.
    /// `None` when there is no Wayland session or no layer-shell.
    pub fn start(on_button: ButtonSink) -> Option<Self> {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let (ready_tx, ready_rx) = mpsc::channel::<bool>();
        std::thread::Builder::new()
            .name("ascuta-bar".into())
            .spawn(move || {
                if let Err(e) = run(rx, ready_tx.clone(), on_button) {
                    eprintln!("ascuta: layer bar stopped: {e}");
                    let _ = ready_tx.send(false);
                }
            })
            .ok()?;
        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(true) => Some(Self {
                tx,
                mode: Mode::Hidden,
            }),
            _ => None,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        let _ = self.tx.send(Cmd::Mode(mode));
    }

    pub fn set_level(&mut self, level: f32) {
        let _ = self.tx.send(Cmd::Level(level));
    }

    pub fn show(&mut self, mode: Mode) {
        self.set_mode(mode);
    }
}

struct State {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    shm: Shm,
    pool: SlotPool,
    layer: LayerSurface,
    pointer: Option<wl_pointer::WlPointer>,
    configured: bool,
    scale: i32,
    mode: Mode,
    level: f32,
    smooth: f32,
    started: Instant,
    font: Option<ab_glyph::FontVec>,
    on_button: ButtonSink,
    /// Last mode whose input region was applied.
    region_mode: Option<Mode>,
}

fn run(
    rx: Receiver<Cmd>,
    ready: Sender<bool>,
    on_button: ButtonSink,
) -> Result<(), Box<dyn std::error::Error>> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();

    let compositor = CompositorState::bind(&globals, &qh)?;
    let layer_shell = LayerShell::bind(&globals, &qh)?; // Err → fall back to X11
    let shm = Shm::bind(&globals, &qh)?;

    let surface = compositor.create_surface(&qh);
    let layer =
        layer_shell.create_layer_surface(&qh, surface, Layer::Overlay, Some("ascuta"), None);
    layer.set_anchor(Anchor::BOTTOM);
    layer.set_margin(0, 0, 40, 0);
    layer.set_size(WIN_W as u32, WIN_H as u32);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer.set_exclusive_zone(-1); // never push other windows around
    layer.commit();

    let pool = SlotPool::new((WIN_W * WIN_H) as usize * 4 * 4, &shm)?;
    let mut state = State {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        shm,
        pool,
        layer,
        pointer: None,
        configured: false,
        scale: 1,
        mode: Mode::Hidden,
        level: 0.0,
        smooth: 0.0,
        started: Instant::now(),
        font: crate::sysfont::bytes().and_then(|b| ab_glyph::FontVec::try_from_vec(b).ok()),
        on_button,
        region_mode: None,
    };

    // Wait for the first configure so we know the surface is mapped.
    while !state.configured {
        queue.blocking_dispatch(&mut state)?;
    }
    let _ = ready.send(true);
    state.redraw(&qh);

    let mut last_frame = Instant::now();
    loop {
        // Commands from the main thread.
        let mut dirty = false;
        loop {
            match rx.try_recv() {
                Ok(Cmd::Mode(m)) => {
                    state.mode = m;
                    state.smooth = 0.0;
                    dirty = true;
                }
                Ok(Cmd::Level(l)) => state.level = l,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }
        // Animate while visible (~25 fps); redraw once on mode changes.
        let animating = state.mode != Mode::Hidden;
        if dirty || (animating && last_frame.elapsed() >= Duration::from_millis(40)) {
            state.redraw(&qh);
            last_frame = Instant::now();
        }

        queue.flush()?;
        queue.dispatch_pending(&mut state)?;
        if let Some(guard) = queue.prepare_read() {
            let fd = guard.connection_fd().as_raw_fd();
            let mut pfd = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: valid pollfd, count 1, bounded timeout.
            let n = unsafe { libc::poll(&mut pfd, 1, 15) };
            if n > 0 {
                guard.read()?;
            } else {
                drop(guard);
            }
            queue.dispatch_pending(&mut state)?;
        }
    }
}

impl State {
    fn redraw(&mut self, qh: &QueueHandle<Self>) {
        if !self.configured {
            return;
        }
        let scale = self.scale.max(1);
        let (w, h) = (WIN_W as i32 * scale, WIN_H as i32 * scale);
        let Ok((buffer, canvas)) = self
            .pool
            .create_buffer(w, h, w * 4, wl_shm::Format::Argb8888)
        else {
            return;
        };

        // Render into a u32 buffer, then copy as little-endian ARGB8888
        // (premultiplied — exactly what render_bar produces).
        let mut px = vec![0u32; (w * h) as usize];
        let t = self.started.elapsed().as_secs_f32();
        self.smooth = self.smooth * 0.5 + self.level.clamp(0.0, 1.0) * 0.5;
        let label = match self.mode {
            Mode::Decoding => Some("Transcribing"),
            Mode::Cleaning => Some("Cleaning up"),
            _ => None,
        };
        let labelled = label.is_some() && self.font.is_some();
        render_bar(
            &mut px,
            w as usize,
            h as usize,
            self.mode,
            self.smooth,
            t,
            labelled,
        );
        if let (Some(text), Some(font)) = (label, &self.font) {
            let dots = ".".repeat(1 + (t * 2.0) as usize % 3);
            draw_label(&mut px, w as usize, h as usize, font, text, &dots);
        }
        for (chunk, p) in canvas.as_chunks_mut::<4>().0.iter_mut().zip(px.iter()) {
            chunk.copy_from_slice(&p.to_le_bytes());
        }

        let surface = self.layer.wl_surface();
        surface.set_buffer_scale(scale);
        surface.damage_buffer(0, 0, w, h);
        // Click-through unless the buttons are shown.
        if self.region_mode != Some(self.mode) {
            if let Ok(region) = Region::new(&self.compositor) {
                if self.mode == Mode::Listening {
                    region.add(0, 0, WIN_W as i32, WIN_H as i32);
                }
                surface.set_input_region(Some(region.wl_region()));
            }
            self.region_mode = Some(self.mode);
        }
        let _ = buffer.attach_to(surface);
        self.layer.commit();
        let _ = qh;
    }
}

impl CompositorHandler for State {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        self.scale = new_factor.max(1);
        self.redraw(qh);
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for State {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {}
    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &LayerSurface,
        _: LayerSurfaceConfigure,
        _: u32,
    ) {
        let first = !self.configured;
        self.configured = true;
        if !first {
            self.redraw(qh);
        }
    }
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat_state.get_pointer(qh, &seat).ok();
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(p) = self.pointer.take() {
                p.release();
            }
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for State {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for e in events {
            if &e.surface != self.layer.wl_surface() {
                continue;
            }
            if let PointerEventKind::Press { button: 0x110, .. } = e.kind {
                // Surface-local logical coordinates, same space as `hit_test`.
                if let Some(b) = hit_test(self.mode, e.position.0 as f32, e.position.1 as f32) {
                    (self.on_button)(b);
                }
            }
        }
    }
}

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_compositor!(State);
delegate_output!(State);
delegate_shm!(State);
delegate_seat!(State);
delegate_pointer!(State);
delegate_layer!(State);
delegate_registry!(State);

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}
