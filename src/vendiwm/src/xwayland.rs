// XWayland integration — lets X11-only apps (Discord/Electron, which crash on
// NVIDIA's native-Wayland gbm-scanout path, plus anything else without a
// Wayland backend) run under vendiwm via the Xserver.
//
// Everything dispatches on `State`: the xwayland-shell global (so XWayland can
// tie each X11 window to the wl_surface it renders to) is created on State, and
// `X11Wm::start_wm` runs the X11 window manager on the calloop event loop —
// whose data type is State — so the `XwmHandler` and the `X11Wm` itself
// (`state.xwm`) live here too. smithay ties these together: the xwayland-shell
// commit hook reaches `XwmHandler::xwm_state` on the wayland dispatch type.
//
// X11 windows are wrapped in the same `smithay::desktop::Window` the rest of the
// compositor uses, so they flow through the existing Space, tiling tree, render
// path and IPC unchanged — `window.toplevel()` just returns None for them and
// the wayland-only code paths skip past.

use std::os::unix::io::OwnedFd;

use smithay::{
    desktop::Window,
    utils::{Logical, Rectangle},
    wayland::{
        selection::{
            SelectionTarget,
            data_device::{
                clear_data_device_selection, current_data_device_selection_userdata,
                request_data_device_client_selection, set_data_device_selection,
            },
            primary_selection::{
                clear_primary_selection, current_primary_selection_userdata,
                request_primary_client_selection, set_primary_selection,
            },
        },
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        X11Surface, X11Wm,
        xwm::{Reorder, ResizeEdge as X11ResizeEdge, XwmHandler, XwmId},
    },
};

use crate::state::{State, window_id};

impl XWaylandShellHandler for State {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }
}

// XWayland drag-and-drop bound for `X11Wm::start_wm`. We don't start
// XWayland-initiated DnD grabs (move/resize go through Super+drag), so the
// default no-op methods are all we need; State already provides SeatHandler +
// DataDeviceHandler, satisfying the rest of the bound.
impl smithay::input::dnd::DndGrabHandler for State {}

impl State {
    /// The managed window wrapping a given X11 surface, if it's mapped.
    fn x11_window(&self, surface: &X11Surface) -> Option<Window> {
        self.space
            .elements()
            .find(|w| w.x11_surface() == Some(surface))
            .cloned()
    }

    /// Whether an X11 window should float rather than tile. Mirrors the Wayland
    /// rule: dialogs (transient/modal/utility/splash) and fixed-size windows
    /// (Discord's updater/splash — stretching them to a tile leaves them blank)
    /// belong in the floating layer at their intended size.
    fn x11_should_float(&self, surface: &X11Surface) -> bool {
        use smithay::xwayland::xwm::WmWindowType;
        if surface.is_popup() || surface.is_transient_for().is_some() {
            return true;
        }
        if matches!(
            surface.window_type(),
            Some(
                WmWindowType::Dialog
                    | WmWindowType::Utility
                    | WmWindowType::Toolbar
                    | WmWindowType::Splash
                    | WmWindowType::Menu
                    | WmWindowType::DropdownMenu
                    | WmWindowType::PopupMenu
                    | WmWindowType::Tooltip
                    | WmWindowType::Notification
            )
        ) {
            return true;
        }
        match (surface.min_size(), surface.max_size()) {
            (Some(min), Some(max)) => min.w > 0 && min.h > 0 && min == max,
            _ => false,
        }
    }
}

impl XwmHandler for State {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwm.as_mut().unwrap()
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}
    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Err(e) = surface.set_mapped(true) {
            tracing::warn!(?e, "x11 set_mapped failed");
            return;
        }
        let float = self.x11_should_float(&surface);
        let window = Window::new_x11_window(surface.clone());
        let id = window_id(&window);

        // Same insertion dance as a new Wayland toplevel: into the active
        // workspace's tiling tree, queue the open animation, map, lay out.
        self.workspaces.active().tree.insert(window.clone());
        self.workspaces.active().focus_floating = None;
        self.open_anims.push((window.clone(), None));
        self.space.map_element(window.clone(), (0, 0), true);

        if float {
            self.float_window(&window);
        }
        self.relayout(); // sets size + sends the X11 configure (see State::relayout)
        self.update_keyboard_focus();

        let title = surface.title();
        self.window_titles.insert(id, title.clone());
        self.rule_checked.insert(id);
        self.pending_ipc_events
            .push(crate::ipc::Event::WindowOpened { id, title });
        self.emit_workspaces();
        tracing::info!(?float, "x11 window mapped");
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        // Override-redirect = the client positions itself (menus, tooltips,
        // combo dropdowns, splash, Steam's transient popups). Map verbatim at
        // its requested geometry, on top; never tile, relayout, or steal the
        // keyboard — an earlier "focus OR windows" attempt let Steam's transient
        // popups grab focus away from the running game.
        let location = surface.geometry().loc;
        let window = Window::new_x11_window(surface);
        self.space.map_element(window.clone(), location, true);
        self.space.raise_element(&window, true);
        self.pending_redraw = true;
    }

    fn unmapped_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(window) = self.x11_window(&surface) {
            let id = window_id(&window);
            let geo = self
                .space
                .element_geometry(&window)
                .or_else(|| self.last_geos.get(&id).copied());
            if let Some(geo) = geo {
                self.closing.push((id, geo));
            }
            self.workspaces.remove_window(&window);
            self.space.unmap_elem(&window);
            self.window_titles.remove(&id);
            self.last_geos.remove(&id);
            self.rule_checked.remove(&id);
            self.pending_ipc_events
                .push(crate::ipc::Event::WindowClosed { id });
        }
        if !surface.is_override_redirect() {
            let _ = surface.set_mapped(false);
        }
        self.relayout();
        self.update_keyboard_focus();
        self.emit_workspaces();
    }

    fn destroyed_window(&mut self, _xwm: XwmId, _surface: X11Surface) {}

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        _x: Option<i32>,
        _y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        // Tiled X11 windows don't move/resize themselves — the layout owns their
        // geometry, so re-ack the size they have. Floating / not-yet-mapped
        // windows may take the requested size.
        let mut geo = surface.geometry();
        let floating = self
            .x11_window(&surface)
            .map(|win| {
                self.workspaces
                    .active_ref()
                    .floating
                    .iter()
                    .any(|(w, _)| w == &win)
            })
            .unwrap_or(true);
        if floating {
            if let Some(w) = w {
                geo.size.w = w as i32;
            }
            if let Some(h) = h {
                geo.size.h = h as i32;
            }
        }
        let _ = surface.configure(geo);
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
        // Follow self-positioned (override-redirect) moves; tiled/floating
        // geometry is owned by relayout.
        if surface.is_override_redirect() {
            if let Some(window) = self.x11_window(&surface) {
                self.space.map_element(window, geometry.loc, false);
                self.pending_redraw = true;
            }
        }
    }

    fn maximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        let _ = surface.set_maximized(true);
    }
    fn unmaximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        let _ = surface.set_maximized(false);
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(window) = self.x11_window(&surface) {
            let _ = surface.set_fullscreen(true);
            self.set_fullscreen(&window, true);
        }
    }
    fn unfullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(window) = self.x11_window(&surface) {
            let _ = surface.set_fullscreen(false);
            self.set_fullscreen(&window, false);
        }
    }

    // vendiwm moves/resizes via Super+drag, not client grabs. Honour the
    // request minimally by floating the window; the user drives the rest.
    fn resize_request(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        _button: u32,
        _edges: X11ResizeEdge,
    ) {
        if let Some(window) = self.x11_window(&surface) {
            self.float_window(&window);
        }
    }
    fn move_request(&mut self, _xwm: XwmId, surface: X11Surface, _button: u32) {
        if let Some(window) = self.x11_window(&surface) {
            self.float_window(&window);
        }
    }

    // ── selection (clipboard / primary) bridging ───────────────────────────
    fn send_selection(
        &mut self,
        _xwm: XwmId,
        selection: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
    ) {
        match selection {
            SelectionTarget::Clipboard => {
                if let Err(err) = request_data_device_client_selection(&self.seat, mime_type, fd) {
                    tracing::error!(?err, "x11: failed to read wayland clipboard");
                }
            }
            SelectionTarget::Primary => {
                if let Err(err) = request_primary_client_selection(&self.seat, mime_type, fd) {
                    tracing::error!(?err, "x11: failed to read wayland primary selection");
                }
            }
        }
    }

    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        match selection {
            SelectionTarget::Clipboard => {
                set_data_device_selection(&self.display_handle, &self.seat, mime_types, ())
            }
            SelectionTarget::Primary => {
                set_primary_selection(&self.display_handle, &self.seat, mime_types, ())
            }
        }
    }

    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        match selection {
            SelectionTarget::Clipboard => {
                if current_data_device_selection_userdata(&self.seat).is_some() {
                    clear_data_device_selection(&self.display_handle, &self.seat)
                }
            }
            SelectionTarget::Primary => {
                if current_primary_selection_userdata(&self.seat).is_some() {
                    clear_primary_selection(&self.display_handle, &self.seat)
                }
            }
        }
    }

    fn disconnected(&mut self, _xwm: XwmId) {
        tracing::warn!("xwm disconnected");
        self.xwm = None;
    }
}
