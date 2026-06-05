// Core compositor state. Holds every Wayland global vendiwm currently exposes,
// plus the trait impls smithay needs to dispatch protocol messages.
//
// Modeled after smithay's `examples/minimal.rs` (the canonical reference for
// the current API), but split out so backends share the same State.

use smithay::{
    input::{
        Seat, SeatHandler, SeatState,
        pointer::CursorImageStatus,
    },
    reexports::wayland_server::{
        Client,
        backend::{ClientData, ClientId, DisconnectReason},
        protocol::{wl_buffer, wl_seat, wl_surface::WlSurface},
    },
    utils::Serial,
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState,
        },
        dmabuf::{DmabufHandler, DmabufState, DmabufGlobal, ImportNotifier},
        selection::{
            SelectionHandler,
            data_device::{DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler},
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface,
            XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState},
    },
    backend::allocator::dmabuf::Dmabuf,
};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::backend::renderer::utils::on_commit_buffer_handler;

pub struct State {
    pub compositor_state:   CompositorState,
    pub xdg_shell_state:    XdgShellState,
    pub shm_state:          ShmState,
    pub seat_state:         SeatState<Self>,
    pub data_device_state:  DataDeviceState,
    pub dmabuf_state:       DmabufState,
    pub seat:               Seat<Self>,
    // Queued dmabuf imports — the backend drains and processes these each
    // frame, where it has &mut access to the renderer.
    pub pending_dmabuf_imports: Vec<(Dmabuf, ImportNotifier)>,
}

// ── per-client data ──────────────────────────────────────────────────────────
#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {
        tracing::debug!("client initialized");
    }
    fn disconnected(&self, _client_id: ClientId, reason: DisconnectReason) {
        tracing::debug!(?reason, "client disconnected");
    }
}

// ── handler impls ────────────────────────────────────────────────────────────

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }
    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }
    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
    }
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState { &self.shm_state }
}

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }
    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // Mark new toplevels Activated so clients render at full opacity.
        // Real layout placement lands in the tiling tree (next iteration).
        surface.with_pending_state(|s| { s.states.set(xdg_toplevel::State::Activated); });
        surface.send_configure();
        tracing::info!("new toplevel");
    }
    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {}
    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}
    fn reposition_request(&mut self, _surface: PopupSurface, _positioner: PositionerState, _token: u32) {}
}

impl SelectionHandler for State {
    type SelectionUserData = ();
}

impl DataDeviceHandler for State {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.data_device_state
    }
}
impl WaylandDndGrabHandler for State {}

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus  = WlSurface;
    type TouchFocus    = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> { &mut self.seat_state }
    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}
    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {}
}

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }
    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        // Defer to the backend — it owns the renderer and processes these
        // in the event loop after dispatch returns.
        self.pending_dmabuf_imports.push((dmabuf, notifier));
    }
}

// Wires up Dispatch for every Wayland global the handlers above implement.
smithay::delegate_dispatch2!(State);
