// Core compositor state. Holds every Wayland global vendiwm currently exposes,
// plus the trait impls smithay needs to dispatch protocol messages.
//
// Modeled after smithay's `examples/minimal.rs` (the canonical reference for
// the current API), but split out so backends share the same State.

use crate::config::Config;
use crate::layout::Tree;
use smithay::{
    desktop::{LayerSurface, PopupManager, Space, Window, WindowSurfaceType, layer_map_for_output},
    input::{
        Seat, SeatHandler, SeatState,
        pointer::CursorImageStatus,
    },
    utils::{Logical, Point, SERIAL_COUNTER, Serial},
    reexports::wayland_server::{
        Client,
        backend::{ClientData, ClientId, DisconnectReason},
        protocol::{wl_buffer, wl_output, wl_seat, wl_surface::WlSurface},
    },
    wayland::{
        seat::WaylandFocus,
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState,
        },
        dmabuf::{DmabufHandler, DmabufState, DmabufGlobal, ImportNotifier},
        output::{OutputHandler, OutputManagerState},
        selection::{
            SelectionHandler,
            data_device::{DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler},
        },
        shell::{
            xdg::{
                PopupSurface, PositionerState, ToplevelSurface,
                XdgShellHandler, XdgShellState,
            },
            wlr_layer::{
                Layer, LayerSurface as WlrLayerSurface, WlrLayerShellHandler,
                WlrLayerShellState,
            },
        },
        shm::{ShmHandler, ShmState},
    },
    backend::allocator::dmabuf::Dmabuf,
    output::Output,
};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::backend::renderer::utils::on_commit_buffer_handler;

pub struct State {
    pub compositor_state:      CompositorState,
    pub xdg_shell_state:       XdgShellState,
    pub shm_state:             ShmState,
    pub seat_state:            SeatState<Self>,
    pub data_device_state:     DataDeviceState,
    pub dmabuf_state:          DmabufState,
    pub layer_shell_state:     WlrLayerShellState,
    pub output_manager_state:  OutputManagerState,
    pub seat:                  Seat<Self>,

    // Unified window manager — handles toplevels, popups, layer-shell rendering,
    // multi-output stacking, focus stack. Tiling layout layers on top of this.
    pub space:                 Space<Window>,
    pub popups:                PopupManager,

    // i3-style split tree. We update window positions in `space` from this
    // tree after every change. Per-workspace trees come later (v0.2+).
    pub layout:                Tree,

    // Compiled keybinds + future settings, loaded at startup from KDL.
    pub config:                Config,

    // Current pointer position in compositor logical coordinates.
    pub pointer_location:      Point<f64, Logical>,

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
        // Mark Activated so clients render at full opacity, then insert into
        // the tiling tree and relayout. relayout() pushes the new positions
        // into `space` and sends configure to every affected window.
        surface.with_pending_state(|s| { s.states.set(xdg_toplevel::State::Activated); });
        surface.send_configure();
        let window = Window::new_wayland_window(surface);
        self.layout.insert(window.clone());
        self.space.map_element(window, (0, 0), true);
        self.relayout();
        tracing::info!("new toplevel inserted");
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

impl OutputHandler for State {}

impl WlrLayerShellHandler for State {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }
    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        wl_output: Option<wl_output::WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        // Attach to the named output (or the first available one).
        let output = wl_output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.space.outputs().next().cloned());
        let Some(output) = output else {
            tracing::warn!("layer surface with no output — dropping");
            return;
        };
        let mut map = layer_map_for_output(&output);
        if let Err(e) = map.map_layer(&LayerSurface::new(surface, namespace.clone())) {
            tracing::warn!(?e, "map_layer failed");
        } else {
            tracing::info!(%namespace, "new layer surface");
        }
    }
    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let outputs: Vec<_> = self.space.outputs().cloned().collect();
        for output in outputs {
            let mut map = layer_map_for_output(&output);
            let layer = map.layers()
                .find(|l| l.layer_surface() == &surface)
                .cloned();
            if let Some(layer) = layer {
                map.unmap_layer(&layer);
                tracing::info!("layer destroyed");
                return;
            }
        }
    }
}

impl State {
    /// Find the topmost wl_surface under the given point, along with its
    /// absolute logical position. Used for pointer focus and click routing.
    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        // Currently only checks tiled toplevels. Layer-shell + popups + fullscreen
        // are added when those protocols land.
        let (window, loc) = self.space.element_under(pos)?;
        let (surface, surf_loc) = window.surface_under(pos - loc.to_f64(), WindowSurfaceType::ALL)?;
        Some((surface, (surf_loc + loc).to_f64()))
    }

    /// Click-to-focus: set keyboard focus to the surface under the cursor.
    /// No-op if pointer isn't over any client surface.
    pub fn focus_window_at_cursor(&mut self) {
        let Some((surf, _)) = self.surface_under(self.pointer_location) else { return };
        // Raise the matching window in space (clone to drop the iter borrow).
        let target = self.space.elements()
            .find(|w| w.wl_surface().map(|s| *s == surf).unwrap_or(false))
            .cloned();
        if let Some(window) = target {
            self.space.raise_element(&window, true);
        }
        if let Some(kb) = self.seat.get_keyboard() {
            kb.set_focus(self, Some(surf), SERIAL_COUNTER.next_serial());
        }
    }

    /// Execute a keybind action. Returns true if the caller should exit the
    /// main loop (Action::Quit).
    pub fn run_action(&mut self, action: crate::input::Action) -> bool {
        use crate::input::Action::*;
        match action {
            Spawn(cmd) => {
                tracing::info!(%cmd, "spawn");
                // Detach so vendiwm doesn't accumulate zombies.
                if let Err(e) = std::process::Command::new("sh").arg("-c").arg(&cmd).spawn() {
                    tracing::warn!(?e, %cmd, "spawn failed");
                }
            }
            Close => {
                if let Some(w) = self.layout.focused().cloned() {
                    if let Some(t) = w.toplevel() {
                        t.send_close();
                    }
                }
            }
            FocusNext => { self.layout.focus_next(); }
            FocusPrev => { self.layout.focus_prev(); }
            SetNextSplit(dir) => { self.layout.next_split_override = Some(dir); }
            Quit => return true,
        }
        false
    }

    /// Walk the layout tree and push each window's computed rectangle into
    /// the space + send xdg_toplevel.configure so the client resizes.
    pub fn relayout(&mut self) {
        // Drop any windows whose clients have died — they'd leave a hole
        // in the tree otherwise.
        self.layout.prune_dead();
        // Use the first output's geometry as the tile viewport. Once we have
        // per-monitor workspaces this becomes "the output for this workspace".
        let Some(output) = self.space.outputs().next().cloned() else { return };
        let geometry = match self.space.output_geometry(&output) {
            Some(g) => g,
            None => return,
        };
        for (window, rect) in self.layout.layout(geometry) {
            // Send size to the client via xdg_toplevel.configure.
            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|s| { s.size = Some(rect.size); });
                toplevel.send_pending_configure();
            }
            // Map at the computed location. `false` = don't activate (focus
            // is managed by the layout tree, not by map ordering).
            self.space.map_element(window, rect.loc, false);
        }
    }
}

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
