// vendi-demo — minimal Wayland client.
//
// Connects to whatever compositor $WAYLAND_DISPLAY points at, opens an
// xdg_toplevel filled with Catppuccin Mauve via wl_shm, runs until closed.
//
// Smallest possible proof that a compositor can host real app windows.
// Built on smithay-client-toolkit, modeled on its simple_window example
// but stripped of input/keyboard/pointer handling.

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_output, delegate_registry, delegate_shm,
    delegate_xdg_shell, delegate_xdg_window,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        xdg::{
            XdgShell,
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
        },
    },
    shm::{
        Shm, ShmHandler,
        slot::SlotPool,
    },
};
use smithay_client_toolkit::reexports::client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_output, wl_shm, wl_surface},
};
use smithay_client_toolkit::reexports::calloop::EventLoop;
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;

const W: u32 = 480;
const H: u32 = 320;

// Catppuccin Mauve #cba6f7
const FILL: [u8; 4] = [0xf7, 0xa6, 0xcb, 0xff];  // BGRA (xrgb8888 little-endian)

fn main() {
    let conn = Connection::connect_to_env().expect("WAYLAND_DISPLAY not set or invalid");

    let (globals, event_queue) = registry_queue_init(&conn).unwrap();
    let qh = event_queue.handle();
    let mut event_loop: EventLoop<App> = EventLoop::try_new().unwrap();
    WaylandSource::new(conn.clone(), event_queue).insert(event_loop.handle()).unwrap();

    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor");
    let xdg_shell  = XdgShell::bind(&globals, &qh).expect("xdg_wm_base");
    let shm        = Shm::bind(&globals, &qh).expect("wl_shm");

    let surface = compositor.create_surface(&qh);
    let window  = xdg_shell.create_window(surface, WindowDecorations::RequestServer, &qh);
    window.set_title("vendi-demo");
    window.set_app_id("dev.vendi.demo");
    window.set_min_size(Some((W, H)));
    // Initial commit — kicks off the configure round-trip.
    window.commit();

    let pool = SlotPool::new((W * H * 4) as usize, &shm).expect("slot pool");

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state:   OutputState::new(&globals, &qh),
        shm,
        window,
        pool,
        w: W, h: H,
        exit: false,
        first_configure: true,
    };

    while !app.exit {
        event_loop.dispatch(None, &mut app).unwrap();
    }
}

struct App {
    registry_state:   RegistryState,
    output_state:     OutputState,
    shm:              Shm,
    window:           Window,
    pool:             SlotPool,
    w:                u32,
    h:                u32,
    exit:             bool,
    first_configure:  bool,
}

impl App {
    fn draw(&mut self, qh: &QueueHandle<Self>) {
        let stride = (self.w * 4) as i32;
        let (buffer, canvas) = self.pool
            .create_buffer(self.w as i32, self.h as i32, stride, wl_shm::Format::Xrgb8888)
            .expect("create buffer");
        // Fill — BGRA bytes for xrgb8888.
        for px in canvas.chunks_exact_mut(4) {
            px.copy_from_slice(&FILL);
        }
        self.window
            .wl_surface()
            .damage_buffer(0, 0, self.w as i32, self.h as i32);
        self.window.wl_surface().frame(qh, self.window.wl_surface().clone());
        buffer.attach_to(self.window.wl_surface()).expect("attach buffer");
        self.window.commit();
    }
}

// ── compositor handler (frame callbacks) ─────────────────────────────────────
impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _s: &wl_surface::WlSurface, _scale: i32) {}
    fn transform_changed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _s: &wl_surface::WlSurface, _t: wl_output::Transform) {}
    fn frame(&mut self, _conn: &Connection, qh: &QueueHandle<Self>, _surface: &wl_surface::WlSurface, _time: u32) {
        self.draw(qh);
    }
    fn surface_enter(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _surface: &wl_surface::WlSurface, _output: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _surface: &wl_surface::WlSurface, _output: &wl_output::WlOutput) {}
}

// ── xdg window handler (configure / close) ───────────────────────────────────
impl WindowHandler for App {
    fn request_close(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _window: &Window) {
        self.exit = true;
    }
    fn configure(&mut self, _conn: &Connection, qh: &QueueHandle<Self>, _window: &Window, configure: WindowConfigure, serial: u32) {
        let old_w = self.w;
        let old_h = self.h;
        let nw = configure.new_size.0.map(u32::from);
        let nh = configure.new_size.1.map(u32::from);
        eprintln!("[demo] configure serial={serial} new_size=({nw:?},{nh:?})");
        if let Some(size) = configure.new_size.0 { self.w = size.into(); }
        if let Some(size) = configure.new_size.1 { self.h = size.into(); }
        if self.w == 0 { self.w = W; }
        if self.h == 0 { self.h = H; }
        eprintln!("[demo] drawing at {}x{}", self.w, self.h);
        if self.first_configure || self.w != old_w || self.h != old_h {
            self.first_configure = false;
            self.draw(qh);
        }
    }
}

// ── output / shm / registry handlers ─────────────────────────────────────────
impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState { &mut self.output_state }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm { &mut self.shm }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState { &mut self.registry_state }
    registry_handlers![OutputState];
}

delegate_compositor!(App);
delegate_output!(App);
delegate_shm!(App);
delegate_xdg_shell!(App);
delegate_xdg_window!(App);
delegate_registry!(App);
