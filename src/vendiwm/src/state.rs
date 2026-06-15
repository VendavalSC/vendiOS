// Core compositor state. Holds every Wayland global vendiwm currently exposes,
// plus the trait impls smithay needs to dispatch protocol messages.
//
// Modeled after smithay's `examples/minimal.rs` (the canonical reference for
// the current API), but split out so backends share the same State.

use std::collections::HashMap;

use crate::config::Config;
use crate::workspaces::Workspaces;
use smithay::{
    desktop::{LayerSurface, PopupManager, Space, Window, WindowSurfaceType, layer_map_for_output},
    input::{
        Seat, SeatHandler, SeatState,
        pointer::CursorImageStatus,
    },
    utils::{IsAlive, Logical, Point, Rectangle, SERIAL_COUNTER, Serial},
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
            with_states,
        },
        dmabuf::{DmabufHandler, DmabufState, DmabufGlobal, ImportNotifier},
        idle_inhibit::{IdleInhibitHandler, IdleInhibitManagerState},
        output::{OutputHandler, OutputManagerState},
        selection::{
            SelectionHandler,
            data_device::{DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler},
            primary_selection::{PrimarySelectionHandler, PrimarySelectionState},
            wlr_data_control::{DataControlHandler, DataControlState},
        },
        viewporter::ViewporterState,
        fractional_scale::{
            FractionalScaleHandler, FractionalScaleManagerState, with_fractional_scale,
        },
        shell::{
            xdg::{
                PopupSurface, PositionerState, ToplevelSurface,
                XdgShellHandler, XdgShellState,
            },
            wlr_layer::{
                Layer, LayerSurface as WlrLayerSurface, LayerSurfaceData,
                WlrLayerShellHandler, WlrLayerShellState,
            },
        },
        shm::{ShmHandler, ShmState},
        session_lock::{
            LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker,
        },
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
    pub session_lock_state:    SessionLockManagerState,
    pub primary_selection_state: PrimarySelectionState,
    /// wlr-data-control — lets clipboard managers (cliphist via wl-paste
    /// --watch) observe and serve the selection.
    pub data_control_state:    DataControlState,
    /// idle-inhibit — surfaces (video players, presentations) that ask the
    /// session to stay awake. While any is active, auto-lock/screen-off pause.
    pub idle_inhibit_state:    IdleInhibitManagerState,
    pub idle_inhibitors:       std::collections::HashSet<WlSurface>,
    pub xdg_decoration_state:  smithay::wayland::shell::xdg::decoration::XdgDecorationState,
    pub viewporter_state:      ViewporterState,
    pub fractional_scale_manager_state: FractionalScaleManagerState,
    pub seat:                  Seat<Self>,

    // ext-session-lock: a client (swaylock) asked to lock. We confirm only
    // after the first locked frame has actually been queued — until then
    // `lock_pending` holds the unconfirmed locker. While locked the renderer
    // shows ONLY the lock surface (black if the client hasn't mapped one).
    pub lock_pending:          Option<SessionLocker>,
    pub locked:                bool,
    pub lock_surface:          Option<LockSurface>,

    // Unified window manager — handles toplevels, popups, layer-shell rendering,
    // multi-output stacking, focus stack. Tiling layout layers on top of this.
    pub space:                 Space<Window>,
    pub popups:                PopupManager,

    // Dynamic workspaces, each owning an i3-style split tree + floating
    // layer. Window positions in `space` are recomputed from the active
    // workspace after every change.
    pub workspaces:            Workspaces,

    // Last known title per window (protocol id) — used to emit
    // window-title IPC events only on actual change.
    pub window_titles:         HashMap<u32, String>,

    // Windows (protocol ids) already run through window rules — each window
    // is classified exactly once, on its first commit that carries an app_id.
    pub rule_checked:          std::collections::HashSet<u32>,

    // Active Super+drag of a floating window (move or resize).
    pub drag:                  Option<Drag>,
    // Just-released drag: the renderer eases the pick-up scale back down.
    pub drag_release:          Option<(Window, std::time::Instant)>,

    // In-flight touchpad swipe: (finger count, accumulated dx, dy).
    // 3 fingers horizontal = workspace switch; 4 fingers vertical = menus.
    pub swipe:                 Option<(u32, f64, f64)>,

    // Overview (exposé): all windows of the active workspace laid out in a
    // centered grid over a dimmed wallpaper. Render-only — the space keeps
    // the real geometry; the backend draws windows at their grid cells.
    // `overview_t` is the last toggle instant (drives the dim transition).
    pub overview:              bool,
    pub overview_t:            std::time::Instant,

    // One-shot screenshot request from IPC: render the next frame to this
    // PNG path (the backend services and clears it).
    pub screenshot:            Option<String>,
    // wlr-screencopy capture requests awaiting their output's next render
    // (wf-recorder, grim, OBS, screen-share portals). Drained by the backend.
    pub pending_screencopy:    Vec<crate::screencopy::PendingScreencopy>,
    // Bumped whenever theme.wallpaper changes at runtime (IPC). Each output
    // surface keeps its own copy and rebuilds its buffer when it lags.
    pub wallpaper_gen:         u64,
    /// vendi-lock — the native lock screen (distinct from the
    /// ext-session-lock fields above, which serve external lockers like
    /// swaylock): while active, rendering shows only the lock screen and
    /// keyboard input feeds the password buffer.
    pub vlock:                 bool,
    pub vlock_input:           String,
    /// Set on a failed unlock attempt — drives the red flash.
    pub vlock_fail:            Option<std::time::Instant>,

    // Last layer-shell non-exclusive zone — relayout only runs when a layer
    // commit actually changes it. Without this, every bar/menu frame would
    // trigger a configure storm to every toplevel.
    pub last_zone:             Option<Rectangle<i32, Logical>>,

    // Idle auto-lock: every input event stamps `last_activity`; a periodic
    // timer locks the session once it exceeds config.idle_lock_secs.
    // `auto_lock_fired` keeps that to one lock per idle stretch.
    pub last_activity:         std::time::Instant,
    pub auto_lock_fired:       bool,
    // True while the displays are powered off via DPMS (idle screen-off).
    // The render loop skips output while set; input clears it and wakes them.
    pub screen_off:            bool,

    // Video screensaver (vendi-screensaver → mpv). When idle crosses the
    // threshold the idle timer spawns the launcher and keeps the child here so
    // any input can kill it. `screensaver` holds the mpv window once it maps
    // (matched by client PID) — the render loop draws it fullscreen above
    // everything. `screensaver_fired` keeps spawning to once per idle stretch.
    pub screensaver_child:     Option<std::process::Child>,
    pub screensaver:           Option<Window>,
    pub screensaver_fired:     bool,
    // When the screensaver window appeared (drives the fade-in) and, once
    // dismissed, when the fade-out began (mpv stays alive until it finishes,
    // so the last frame dissolves instead of snapping away).
    pub screensaver_t:         Option<std::time::Instant>,
    pub screensaver_closing:   Option<std::time::Instant>,

    // Window-open animations. The Instant is None until the window's first
    // frame actually renders — starting the clock at new_toplevel would burn
    // most of the animation during the configure round-trip, so windows
    // popped in half-faded. The render loop starts the clock lazily.
    pub open_anims:            Vec<(Window, Option<std::time::Instant>)>,

    // Workspace-switch animation: (started-at, direction). The new desk
    // fades in and slides from the side it lives on (+1 = from the right).
    pub ws_anim:               Option<(std::time::Instant, i32)>,

    // Layout-morph animations: (window, previous geometry, started-at). The
    // render loop interpolates location AND size, so tile moves, split
    // resizes, and fullscreen toggles all glide instead of snapping.
    pub geo_anims:             Vec<(Window, Rectangle<i32, Logical>, std::time::Instant)>,

    // Windows that just closed: (protocol id, last on-screen geometry). The
    // backend pairs these with its per-frame texture stash and plays a
    // fade-and-shrink. The surface is gone by now — only the texture lives.
    pub closing:               Vec<(u32, Rectangle<i32, Logical>)>,

    // Last rendered geometry per window (protocol id) — the close ghost's
    // fallback when a client unmaps before destroying (Firefox does), at
    // which point the space no longer knows where the window was.
    pub last_geos:             HashMap<u32, Rectangle<i32, Logical>>,

    // Compiled keybinds + future settings, loaded at startup from KDL.
    pub config:                Config,

    // Current pointer position in compositor logical coordinates.
    pub pointer_location:      Point<f64, Logical>,

    // Queued dmabuf imports — the backend drains and processes these each
    // frame, where it has &mut access to the renderer.
    pub pending_dmabuf_imports: Vec<(Dmabuf, ImportNotifier)>,

    // Events the IPC server should push to subscribed clients. The backend
    // drains this each tick and hands it to the IPC server (we keep them as
    // separate concerns rather than letting State own the IPC).
    pub pending_ipc_events:     Vec<crate::ipc::Event>,

    // Set by handlers when something changed that needs the backend to
    // re-render (new toplevel, surface commit, layout change). The udev/winit
    // backend reads + clears this each event-loop tick. Without this, the
    // VBlank-driven render loop stalls after the first empty frame because no
    // page-flip ever queues.
    pub pending_redraw:         bool,

    // Set by the Quit action; the backend's per-tick callback reads this and
    // breaks the calloop event loop.
    pub quit_requested:         bool,
}

/// Computed overview geometry: workspace panels + per-window cells.
#[derive(Default)]
pub struct OverviewLayout {
    /// (workspace id, panel rect, is-active)
    pub panels: Vec<(u32, smithay::utils::Rectangle<i32, smithay::utils::Logical>, bool)>,
    /// (window, cell rect, owning workspace id)
    pub cells: Vec<(Window, smithay::utils::Rectangle<i32, smithay::utils::Logical>, u32)>,
}

/// State of an in-progress Super+drag on a floating window.
#[derive(Clone)]
pub struct Drag {
    pub window:      Window,
    pub resize:      bool,
    // Right-drag on a TILED window: motion adjusts the split ratios instead
    // of a floating rect. `start_ptr` is re-anchored every update.
    pub tile_resize: bool,
    pub start_ptr:   Point<f64, Logical>,
    pub start_rect:  Rectangle<i32, Logical>,
    // When the grab began — the renderer eases in a slight pick-up scale.
    pub started:     std::time::Instant,
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

        // Tell fractional-scale-aware clients (quickshell, alacritty, GTK) the
        // scale to render at. Without this they only ever learn the integer
        // wl_output scale and render blurry/unscaled at fractional scales.
        // set_preferred_scale is a no-op when unchanged / no fractional object.
        if let Some(s) = self.space.outputs().next()
            .map(|o| o.current_scale().fractional_scale())
        {
            with_states(surface, |states| {
                with_fractional_scale(states, |fs| fs.set_preferred_scale(s));
            });
        }

        // Popups: track commits + send the spec-mandated initial configure.
        self.popups.commit(surface);
        if let Some(smithay::desktop::PopupKind::Xdg(popup)) = self.popups.find_popup(surface) {
            if !popup.is_initial_configure_sent() {
                if let Err(e) = popup.send_configure() {
                    tracing::warn!(?e, "popup initial configure failed");
                }
            }
        }
        // Walk up to the root surface and tell the matching Window to
        // recompute its bounding box from the (now committed) surface tree.
        // Without this, Window::geometry() stays (0,0,0,0) and the window
        // never makes it into render output.
        use smithay::wayland::compositor::{get_parent, is_sync_subsurface};
        if is_sync_subsurface(surface) { return; }
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) { root = parent; }
        let window = self.space.elements()
            .find(|w| w.wl_surface().map(|s| *s == root).unwrap_or(false))
            .cloned();
        if let Some(window) = window {
            window.on_commit();

            // The screensaver lives outside the tiling tree, so nothing else
            // schedules a frame for it. Pump one redraw per committed frame so
            // mpv's video composites at its own rate — no busy-loop (which
            // would steal CPU from mpv's software decode), no frozen black.
            if self.screensaver.as_ref() == Some(&window) {
                self.pending_redraw = true;
                // Start the slide-in clock on mpv's FIRST committed frame, not
                // at capture — mpv's startup latency would otherwise eat most
                // of the slide and the video would just appear in place.
                if self.screensaver_t.is_none() {
                    self.screensaver_t = Some(std::time::Instant::now());
                }
            }

            // Title-change detection → IPC event for the bar.
            if let Some(toplevel) = window.toplevel() {
                use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
                let id = window_id(&window);
                let title = with_states(toplevel.wl_surface(), |states| {
                    states.data_map.get::<XdgToplevelSurfaceData>()
                        .and_then(|d| d.lock().ok().and_then(|a| a.title.clone()))
                        .unwrap_or_default()
                });
                if self.window_titles.get(&id) != Some(&title) {
                    self.window_titles.insert(id, title.clone());
                    let focused = self.focused_window().as_ref() == Some(&window);
                    self.pending_ipc_events.push(crate::ipc::Event::WindowTitle { id, title, focused });
                }

                // Window rules — classified once, on the first commit that
                // carries an app_id. `vendi-float` (floating terminals from
                // the menus: About, Install) opens floating, centered.
                if !self.rule_checked.contains(&id) {
                    let app_id = with_states(toplevel.wl_surface(), |states| {
                        states.data_map.get::<XdgToplevelSurfaceData>()
                            .and_then(|d| d.lock().ok().and_then(|a| a.app_id.clone()))
                            .unwrap_or_default()
                    });
                    if !app_id.is_empty() {
                        self.rule_checked.insert(id);
                        if app_id == "vendi-float" {
                            self.float_window(&window);
                        }
                    }
                }
            }
        }

        // ── wlr-layer-shell: send the initial configure on the surface's
        // first commit. The spec mandates this is the trigger; without it,
        // layer-shell clients (waybar, vendibar, mako) sit forever waiting
        // for a configure and never draw. Mirrors anvil's pattern.
        let outputs: Vec<_> = self.space.outputs().cloned().collect();
        for output in outputs {
            let has_layer = {
                let map = layer_map_for_output(&output);
                map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL).is_some()
            };
            if !has_layer { continue; }

            let initial_configure_sent = with_states(surface, |states| {
                states
                    .data_map
                    .get::<LayerSurfaceData>()
                    .map(|d| d.lock().unwrap().initial_configure_sent)
                    .unwrap_or(false)
            });
            // Arrange first so the configure carries the right size.
            let mut map = layer_map_for_output(&output);
            map.arrange();
            if !initial_configure_sent {
                if let Some(layer) = map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL) {
                    layer.layer_surface().send_configure();
                }
            }
            let zone = map.non_exclusive_zone();
            drop(map);
            // Only re-tile + re-focus when this commit changed the usable
            // area (or is the surface's first). Layer clients commit every
            // frame — doing this unconditionally floods every toplevel with
            // configures and tanks performance.
            if !initial_configure_sent || self.last_zone != Some(zone) {
                self.last_zone = Some(zone);
                self.relayout();
                // Launchers set keyboard_interactivity before mapping — grab
                // or release keyboard focus as layer surfaces come and go.
                self.update_keyboard_focus();
            }
            // keyboard_interactivity can also flip on an already-mapped
            // surface (the bar takes the keyboard while its dashboard is
            // expanded, then gives it back) — chase the change without
            // waiting for a zone change. Layer clients commit every frame,
            // so only act when the desired focus actually differs.
            if !self.is_locked() {
                let desired = self.exclusive_layer_surface().or_else(|| {
                    self.focused_window()
                        .filter(|w| w.alive())
                        .and_then(|w| w.wl_surface().map(|s| s.into_owned()))
                });
                if let Some(kb) = self.seat.get_keyboard() {
                    if kb.current_focus() != desired {
                        self.update_keyboard_focus();
                    }
                }
            }
        }

        // New buffer / damage → ask the backend to render this tick.
        self.pending_redraw = true;
    }
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState { &self.shm_state }
}

/// Compositor-assigned unique window id. wl_surface protocol ids are only
/// unique within one client connection — two apps can both be "surface 27" —
/// so every per-window map (focus fades, texture stash, IPC) keys off this.
pub fn window_id(window: &smithay::desktop::Window) -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    struct WindowId(u32);
    window.user_data().insert_if_missing(|| WindowId(NEXT.fetch_add(1, Ordering::Relaxed)));
    window.user_data().get::<WindowId>().unwrap().0
}

/// Per-window opacity override, kept in the window's user-data so it travels
/// with the window and needs no separate map/cleanup. Unset means "use the
/// theme default"; `cycle-opacity` sets it explicitly.
struct WindowOpacity(std::cell::Cell<f32>);

/// Effective opacity for a window: its per-window override if set, else the
/// theme default. Used by the renderer to scale the window's alpha.
pub fn window_opacity(window: &smithay::desktop::Window, theme_default: f32) -> f32 {
    window.user_data()
        .get::<WindowOpacity>()
        .map(|o| o.0.get())
        .unwrap_or(theme_default)
}

/// Set a window's explicit opacity override (clamped to a visible range).
pub fn set_window_opacity(window: &smithay::desktop::Window, value: f32) {
    let v = value.clamp(0.1, 1.0);
    let ud = window.user_data();
    ud.insert_if_missing(|| WindowOpacity(std::cell::Cell::new(v)));
    ud.get::<WindowOpacity>().unwrap().0.set(v);
}

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }
    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // Mark Activated. We defer the configure until relayout has computed
        // the size — otherwise the client draws at its default size and we
        // get a half-filled tile until the next round-trip.
        surface.with_pending_state(|s| { s.states.set(xdg_toplevel::State::Activated); });
        let window = Window::new_wayland_window(surface);
        let id = window_id(&window);
        // Screensaver capture: we spawned vendi-screensaver and are waiting for
        // its window. The first toplevel to map while that's true is it — keep
        // it out of the tiling tree (the render loop draws it fullscreen above
        // everything) so it never flickers in as a tile or steals layout.
        if self.screensaver_child.is_some() && self.screensaver.is_none() {
            let size = self.space.outputs().next()
                .and_then(|o| self.space.output_geometry(o))
                .map(|g| g.size)
                .unwrap_or_else(|| (1920, 1080).into());
            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|s| {
                    s.states.set(xdg_toplevel::State::Fullscreen);
                    s.size = Some(size);
                });
                toplevel.send_configure();
            }
            self.space.map_element(window.clone(), (0, 0), false);
            self.screensaver = Some(window);
            // Slide-in clock is set on mpv's first committed frame (see commit
            // handler), not here — avoids the slide elapsing during startup.
            self.screensaver_t = None;
            self.screensaver_closing = None;
            tracing::info!("screensaver window captured");
            return;
        }
        self.workspaces.active().tree.insert(window.clone());
        self.workspaces.active().focus_floating = None;
        self.open_anims.push((window.clone(), None));
        self.space.map_element(window, (0, 0), true);
        self.relayout();  // sets size in pending state and sends configure.
        self.update_keyboard_focus();
        self.pending_ipc_events.push(crate::ipc::Event::WindowOpened {
            id,
            title: String::new(),
        });
        self.emit_workspaces();
        tracing::info!("new toplevel inserted");
    }
    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        // The screensaver window lives outside the tiling tree; if it died on
        // its own (mpv quit via the q/ESC fallback, or the loop ended), forget
        // it and reap the child so the idle timer can re-arm.
        if self.screensaver.as_ref().and_then(|w| w.toplevel()) == Some(&surface) {
            self.space.unmap_elem(&self.screensaver.take().unwrap());
            if let Some(mut child) = self.screensaver_child.take() {
                let _ = child.wait();
            }
            self.screensaver_t = None;
            self.screensaver_closing = None;
            self.screensaver_fired = false;
            self.last_activity = std::time::Instant::now();
            self.pending_redraw = true;
            return;
        }
        let window = self.workspaces.all_windows().into_iter()
            .find(|w| w.toplevel().map(|t| t == &surface).unwrap_or(false));
        let id = window.as_ref().map(window_id).unwrap_or(0);
        if let Some(window) = window {
            // Last on-screen rect, captured before unmap — the close
            // animation plays a fading ghost there.
            let geo = self.space.element_geometry(&window)
                .or_else(|| self.last_geos.get(&id).copied());
            if let Some(geo) = geo {
                self.closing.push((id, geo));
            }
            self.workspaces.remove_window(&window);
            self.space.unmap_elem(&window);
        }
        self.window_titles.remove(&id);
        self.last_geos.remove(&id);
        self.rule_checked.remove(&id);
        self.pending_ipc_events.push(crate::ipc::Event::WindowClosed { id });
        self.relayout();
        self.update_keyboard_focus();
        self.emit_workspaces();
    }
    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<wl_output::WlOutput>) {
        let window = self.workspaces.all_windows().into_iter()
            .find(|w| w.toplevel().map(|t| t == &surface).unwrap_or(false));
        if let Some(window) = window {
            self.set_fullscreen(&window, true);
        }
    }
    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        let window = self.workspaces.all_windows().into_iter()
            .find(|w| w.toplevel().map(|t| t == &surface).unwrap_or(false));
        if let Some(window) = window {
            self.set_fullscreen(&window, false);
        }
    }
    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        if let Err(e) = self.popups.track_popup(smithay::desktop::PopupKind::from(surface)) {
            tracing::warn!(?e, "track popup failed");
        }
    }
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

impl DataControlHandler for State {
    fn data_control_state(&mut self) -> &mut DataControlState {
        &mut self.data_control_state
    }
}

impl IdleInhibitHandler for State {
    fn inhibit(&mut self, surface: WlSurface) {
        self.idle_inhibitors.insert(surface);
    }
    fn uninhibit(&mut self, surface: WlSurface) {
        self.idle_inhibitors.remove(&surface);
    }
}

impl State {
    /// True while some surface is holding an idle inhibitor (a playing video,
    /// a presentation) — auto-lock and screen-off pause. smithay calls
    /// `uninhibit` when an inhibitor is destroyed, so the set stays accurate.
    pub fn idle_inhibited(&self) -> bool {
        !self.idle_inhibitors.is_empty()
    }
}

impl PrimarySelectionHandler for State {
    fn primary_selection_state(&mut self) -> &mut PrimarySelectionState {
        &mut self.primary_selection_state
    }
}

// xdg-decoration: vendiwm always draws the chrome (borders/rings) itself, so
// every toplevel is told ServerSide — clients skip their own titlebars.
impl smithay::wayland::shell::xdg::decoration::XdgDecorationHandler for State {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
        toplevel.with_pending_state(|s| {
            s.decoration_mode = Some(Mode::ServerSide);
        });
        toplevel.send_configure();
    }
    fn request_mode(
        &mut self,
        toplevel: ToplevelSurface,
        _mode: smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode,
    ) {
        use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
        toplevel.with_pending_state(|s| {
            s.decoration_mode = Some(Mode::ServerSide);
        });
        toplevel.send_pending_configure();
    }
    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
        toplevel.with_pending_state(|s| {
            s.decoration_mode = Some(Mode::ServerSide);
        });
        toplevel.send_pending_configure();
    }
}

impl OutputHandler for State {}

impl FractionalScaleHandler for State {
    // A client just bound wp_fractional_scale for this surface — send it the
    // current preferred scale right away (the commit handler keeps it fresh).
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        if let Some(s) = self.space.outputs().next()
            .map(|o| o.current_scale().fractional_scale())
        {
            with_states(&surface, |states| {
                with_fractional_scale(states, |fs| fs.set_preferred_scale(s));
            });
        }
    }
}

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
        let result = map.map_layer(&LayerSurface::new(surface, namespace.clone()));
        // Re-run the anchor + exclusive-zone math so the new layer takes
        // effect immediately. Drop the map borrow first since relayout will
        // re-take it.
        map.arrange();
        drop(map);
        match result {
            Err(e) => tracing::warn!(?e, "map_layer failed"),
            Ok(_) => {
                tracing::info!(%namespace, "new layer surface");
                self.relayout();
            }
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
                map.arrange();
                drop(map);
                tracing::info!("layer destroyed");
                self.relayout();
                // Hand keyboard focus back to the focused window if this
                // layer (e.g. the launcher) was holding it.
                self.update_keyboard_focus();
                return;
            }
        }
    }
}

impl SessionLockHandler for State {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.session_lock_state
    }
    fn lock(&mut self, confirmation: SessionLocker) {
        tracing::info!("session lock requested");
        self.lock_pending = Some(confirmation);
        self.pending_redraw = true;
    }
    fn unlock(&mut self) {
        tracing::info!("session unlocked");
        self.locked = false;
        self.lock_pending = None;
        self.lock_surface = None;
        self.update_keyboard_focus();
        self.pending_redraw = true;
    }
    fn new_surface(&mut self, surface: LockSurface, output: wl_output::WlOutput) {
        let size = Output::from_resource(&output)
            .and_then(|o| self.space.output_geometry(&o))
            .map(|g| g.size)
            .unwrap_or_else(|| (1920, 1080).into());
        surface.with_pending_state(|s| {
            s.size = Some((size.w as u32, size.h as u32).into());
        });
        surface.send_configure();
        self.lock_surface = Some(surface);
        self.update_keyboard_focus();
        self.pending_redraw = true;
    }
}

impl State {
    /// True from the moment a lock is requested until the client unlocks.
    /// The renderer and input paths must show/route ONLY the lock surface.
    pub fn is_locked(&self) -> bool {
        self.locked || self.lock_pending.is_some()
    }

    /// Find the topmost wl_surface under the given point, along with its
    /// absolute logical position. Checks layer surfaces above windows
    /// (Overlay/Top — e.g. the bar), then windows, then lower layers.
    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        // While locked, pointer input may only ever reach the lock surface.
        if self.is_locked() {
            return self.lock_surface.as_ref().map(|l| (l.wl_surface().clone(), Point::from((0.0, 0.0))));
        }
        fn layer_hit(
            map: &smithay::desktop::LayerMap,
            layer: &LayerSurface,
            out_loc: Point<i32, Logical>,
            pos: Point<f64, Logical>,
        ) -> Option<(WlSurface, Point<f64, Logical>)> {
            let geo = map.layer_geometry(layer)?;
            let layer_loc = (geo.loc + out_loc).to_f64();
            let (surface, surf_loc) = layer.surface_under(pos - layer_loc, WindowSurfaceType::ALL)?;
            Some((surface, surf_loc.to_f64() + layer_loc))
        }
        let output = self.space.outputs().next()?.clone();
        let out_geo = self.space.output_geometry(&output)?;
        {
            let map = layer_map_for_output(&output);
            for l in [Layer::Overlay, Layer::Top] {
                if let Some(layer) = map.layer_under(l, pos - out_geo.loc.to_f64()) {
                    if let Some(hit) = layer_hit(&map, layer, out_geo.loc, pos) { return Some(hit); }
                }
            }
        }
        if let Some((window, loc)) = self.space.element_under(pos) {
            if let Some((surface, surf_loc)) = window.surface_under(pos - loc.to_f64(), WindowSurfaceType::ALL) {
                return Some((surface, (surf_loc + loc).to_f64()));
            }
        }
        let map = layer_map_for_output(&output);
        for l in [Layer::Bottom, Layer::Background] {
            if let Some(layer) = map.layer_under(l, pos - out_geo.loc.to_f64()) {
                if let Some(hit) = layer_hit(&map, layer, out_geo.loc, pos) { return Some(hit); }
            }
        }
        None
    }

    /// The window the active workspace considers focused.
    pub fn focused_window(&self) -> Option<Window> {
        self.workspaces.active_ref().focused_window()
    }

    /// A Top/Overlay layer surface demanding exclusive keyboard input
    /// (launcher, lock screen) — it outranks any window focus.
    fn exclusive_layer_surface(&self) -> Option<WlSurface> {
        use smithay::wayland::shell::wlr_layer::{KeyboardInteractivity, LayerSurfaceCachedState};
        let output = self.space.outputs().next()?;
        let map = layer_map_for_output(output);
        let layer = map.layers().rev().find(|l| {
            if !matches!(l.layer(), Layer::Top | Layer::Overlay) { return false; }
            with_states(l.wl_surface(), |states| {
                states.cached_state.get::<LayerSurfaceCachedState>().current()
                    .keyboard_interactivity != KeyboardInteractivity::None
            })
        })?;
        Some(layer.wl_surface().clone())
    }

    /// Push keyboard focus + xdg Activated state to whatever the active
    /// workspace says is focused, raise it, and tell the bar.
    pub fn update_keyboard_focus(&mut self) {
        // A lock surface owns the keyboard unconditionally while locked.
        if self.is_locked() {
            let surf = self.lock_surface.as_ref().map(|l| l.wl_surface().clone());
            if let Some(kb) = self.seat.get_keyboard() {
                kb.set_focus(self, surf, SERIAL_COUNTER.next_serial());
            }
            self.pending_redraw = true;
            return;
        }
        if let Some(surf) = self.exclusive_layer_surface() {
            if let Some(kb) = self.seat.get_keyboard() {
                kb.set_focus(self, Some(surf), SERIAL_COUNTER.next_serial());
            }
            self.pending_redraw = true;
            return;
        }
        let focused = self.focused_window().filter(|w| w.alive());
        let surf = focused.as_ref().and_then(|w| w.wl_surface().map(|s| s.into_owned()));
        if let Some(kb) = self.seat.get_keyboard() {
            kb.set_focus(self, surf.clone(), SERIAL_COUNTER.next_serial());
        }
        // Activated ring: set on the focused toplevel, clear everywhere else.
        for window in self.workspaces.all_windows() {
            let Some(toplevel) = window.toplevel() else { continue };
            let active = Some(&window) == focused.as_ref();
            toplevel.with_pending_state(|s| {
                if active { s.states.set(xdg_toplevel::State::Activated); }
                else      { s.states.unset(xdg_toplevel::State::Activated); }
            });
            toplevel.send_pending_configure();
        }
        if let Some(window) = &focused {
            self.space.raise_element(window, true);
            let id = window_id(window);
            let title = self.window_titles.get(&id).cloned().unwrap_or_default();
            self.pending_ipc_events.push(crate::ipc::Event::WindowFocused { id, title });
        } else {
            self.pending_ipc_events.push(crate::ipc::Event::WindowFocused { id: 0, title: String::new() });
        }
        self.pending_redraw = true;
    }

    /// Queue a workspaces snapshot event for the bar.
    pub fn emit_workspaces(&mut self) {
        let (active, list) = self.workspaces.snapshot();
        self.pending_ipc_events.push(crate::ipc::Event::WorkspacesChanged {
            active,
            workspaces: list.into_iter()
                .map(|(id, windows)| crate::ipc::WorkspaceInfo { id, focused: id == active, windows })
                .collect(),
        });
    }

    /// Switch the active workspace, hiding the old one's windows.
    pub fn switch_workspace(&mut self, id: u32) {
        if id == self.workspaces.active_id() { return; }
        // Overview doesn't survive a desk change — the slide animation owns
        // the transition from here.
        if self.overview {
            self.overview = false;
            self.overview_t = std::time::Instant::now();
        }
        let dir = if id > self.workspaces.active_id() { 1 } else { -1 };
        let hidden = self.workspaces.switch_to(id);
        for w in hidden { self.space.unmap_elem(&w); }
        self.ws_anim = Some((std::time::Instant::now(), dir));
        self.relayout();
        self.update_keyboard_focus();
        self.emit_workspaces();
    }

    /// Send the focused window to workspace `id` (it lands tiled there).
    pub fn move_focused_to_workspace(&mut self, id: u32) {
        let Some(window) = self.focused_window() else { return };
        self.workspaces.move_window_to(&window, id);
        if id != self.workspaces.active_id() {
            self.space.unmap_elem(&window);
        }
        self.relayout();
        self.update_keyboard_focus();
        self.emit_workspaces();
    }

    /// Toggle the focused window between tiled and floating. Floating
    /// windows open centered at ~60% of the viewport.
    pub fn toggle_floating(&mut self) {
        let Some(window) = self.focused_window() else { return };
        let ws = self.workspaces.active();
        if let Some(pos) = ws.floating.iter().position(|(w, _)| w == &window) {
            ws.floating.remove(pos);
            ws.focus_floating = None;
            ws.tree.insert(window);
            self.relayout();
            self.update_keyboard_focus();
        } else {
            self.float_window(&window);
        }
    }

    /// Pop a tiled window out into the floating layer, centered at ~60% of
    /// the viewport. Also used by the `vendi-float` window rule.
    pub fn float_window(&mut self, window: &Window) {
        let Some(vp) = self.tiling_viewport() else { return };
        let size: smithay::utils::Size<i32, Logical> =
            ((vp.size.w * 3 / 5).max(320), (vp.size.h * 3 / 5).max(240)).into();
        let loc: Point<i32, Logical> = (
            vp.loc.x + (vp.size.w - size.w) / 2,
            vp.loc.y + (vp.size.h - size.h) / 2,
        ).into();
        self.float_window_at(window, Rectangle::new(loc, size));
    }

    /// Pop a tiled window out into the floating layer at a specific rect —
    /// Super+LeftDrag uses the window's current geometry so it detaches in
    /// place and follows the cursor.
    pub fn float_window_at(&mut self, window: &Window, rect: Rectangle<i32, Logical>) {
        let ws = self.workspaces.active();
        if !ws.tree.contains(window) { return; }
        ws.tree.remove(window);
        ws.floating.push((window.clone(), rect));
        ws.focus_floating = Some(window.clone());
        self.relayout();
        self.update_keyboard_focus();
    }

    /// Fullscreen on/off for a specific window.
    pub fn set_fullscreen(&mut self, window: &Window, on: bool) {
        let ws = self.workspaces.active();
        if on {
            ws.fullscreen = Some(window.clone());
        } else if ws.fullscreen.as_ref() == Some(window) {
            ws.fullscreen = None;
        }
        if let Some(toplevel) = window.toplevel() {
            toplevel.with_pending_state(|s| {
                if on { s.states.set(xdg_toplevel::State::Fullscreen); }
                else  { s.states.unset(xdg_toplevel::State::Fullscreen); }
            });
        }
        self.relayout();
    }

    /// Tear down a running screensaver immediately: kill mpv and forget its
    /// window. Used when there's no time to fade (screen-off, or mpv died on
    /// its own). Returns true if anything was actually torn down.
    pub fn dismiss_screensaver(&mut self) -> bool {
        if self.screensaver_child.is_none() && self.screensaver.is_none() {
            return false;
        }
        if let Some(mut child) = self.screensaver_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Unmap from the space immediately. Otherwise the window lingers there
        // (until the client disconnect is processed) and, no longer being the
        // tracked screensaver, the normal window loop would render it full-screen
        // for a few frames — the post-dismiss flash.
        if let Some(w) = self.screensaver.take() {
            self.space.unmap_elem(&w);
        }
        self.screensaver_t = None;
        self.screensaver_closing = None;
        self.screensaver_fired = false;
        self.pending_redraw = true;
        true
    }

    /// Begin dismissing the screensaver on input: start the fade-out (mpv keeps
    /// playing so its last frame dissolves) rather than snapping it away. The
    /// render loop kills mpv once the fade completes. Returns true if a
    /// screensaver was up, so the caller can swallow that first input event.
    pub fn begin_screensaver_dismiss(&mut self) -> bool {
        if self.screensaver.is_none() && self.screensaver_child.is_none() {
            return false;
        }
        // Grace period: ignore the keypress that launched it (and any stray
        // event right as it appears) so it doesn't dismiss the instant it
        // shows. Until the window has actually mapped, stay in grace too.
        match self.screensaver_t {
            Some(t0) if t0.elapsed().as_millis() >= 700 => {}
            _ => return false,
        }
        if self.screensaver_closing.is_none() {
            self.screensaver_closing = Some(std::time::Instant::now());
            self.pending_redraw = true;
        }
        true
    }

    pub fn toggle_fullscreen(&mut self) {
        let Some(window) = self.focused_window() else { return };
        let on = self.workspaces.active_ref().fullscreen.as_ref() != Some(&window);
        self.set_fullscreen(&window, on);
    }

    /// Geometric nearest-neighbor in a screen direction, among the active
    /// workspace's windows as currently mapped.
    fn window_in_dir(&self, dir: crate::input::Dir) -> Option<Window> {
        use crate::input::Dir;
        let focused = self.focused_window()?;
        let fgeo = self.space.element_geometry(&focused)?;
        let fc = (fgeo.loc.x + fgeo.size.w / 2, fgeo.loc.y + fgeo.size.h / 2);
        let mut best: Option<(i64, Window)> = None;
        for window in self.workspaces.active_ref().windows() {
            if window == focused { continue; }
            let Some(geo) = self.space.element_geometry(&window) else { continue };
            let c = (geo.loc.x + geo.size.w / 2, geo.loc.y + geo.size.h / 2);
            let (dx, dy) = ((c.0 - fc.0) as i64, (c.1 - fc.1) as i64);
            let in_dir = match dir {
                Dir::Left  => dx < 0 && dx.abs() >= dy.abs(),
                Dir::Right => dx > 0 && dx.abs() >= dy.abs(),
                Dir::Up    => dy < 0 && dy.abs() >= dx.abs(),
                Dir::Down  => dy > 0 && dy.abs() >= dx.abs(),
            };
            if !in_dir { continue; }
            let dist = dx * dx + dy * dy;
            if best.as_ref().map(|(d, _)| dist < *d).unwrap_or(true) {
                best = Some((dist, window));
            }
        }
        best.map(|(_, w)| w)
    }

    pub fn focus_dir(&mut self, dir: crate::input::Dir) {
        let Some(target) = self.window_in_dir(dir) else { return };
        let ws = self.workspaces.active();
        if ws.floating.iter().any(|(w, _)| w == &target) {
            ws.focus_floating = Some(target);
        } else {
            ws.focus_floating = None;
            ws.tree.focus_window(&target);
        }
        self.update_keyboard_focus();
    }

    pub fn move_dir(&mut self, dir: crate::input::Dir) {
        let Some(focused) = self.focused_window() else { return };
        let ws = self.workspaces.active();
        if ws.floating.iter().any(|(w, _)| w == &focused) { return; }
        let Some(target) = self.window_in_dir(dir) else { return };
        let ws = self.workspaces.active();
        if ws.floating.iter().any(|(w, _)| w == &target) { return; }
        ws.tree.swap_windows(&focused, &target);
        ws.tree.focus_window(&focused);
        self.relayout();
    }

    pub fn resize_dir(&mut self, dir: crate::input::Dir) {
        use crate::input::Dir;
        // Focused floating window: grow/shrink the rect directly.
        if let Some(focused) = self.focused_window() {
            let ws = self.workspaces.active();
            if let Some(entry) = ws.floating.iter_mut().find(|(w, _)| w == &focused) {
                const STEP: i32 = 48;
                match dir {
                    Dir::Right => entry.1.size.w += STEP,
                    Dir::Left  => entry.1.size.w = (entry.1.size.w - STEP).max(160),
                    Dir::Down  => entry.1.size.h += STEP,
                    Dir::Up    => entry.1.size.h = (entry.1.size.h - STEP).max(120),
                }
                let (window, rect) = (entry.0.clone(), entry.1);
                // Glide to the new size instead of snapping.
                self.push_geo_anim(&window, rect);
                if let Some(toplevel) = window.toplevel() {
                    toplevel.with_pending_state(|s| { s.size = Some(rect.size); });
                    toplevel.send_pending_configure();
                }
                self.space.map_element(window, rect.loc, true);
                self.pending_redraw = true;
                return;
            }
        }
        let delta = match dir { Dir::Right | Dir::Down => 0.04, Dir::Left | Dir::Up => -0.04 };
        self.workspaces.active().tree.resize_focused(dir.axis(), delta);
        self.relayout();
    }

    /// Update the dragged window from the current pointer position.
    pub fn drag_update(&mut self) {
        let Some(drag) = self.drag.clone() else { return };

        // Tiled right-drag: trade split ratios with the neighbors, KDE-style.
        if drag.tile_resize {
            let Some(vp) = self.tiling_viewport() else { return };
            let dx = self.pointer_location.x - drag.start_ptr.x;
            let dy = self.pointer_location.y - drag.start_ptr.y;
            if let Some(d) = self.drag.as_mut() { d.start_ptr = self.pointer_location; }
            let tree = &mut self.workspaces.active().tree;
            tree.focus_window(&drag.window);
            if dx.abs() >= 1.0 {
                tree.resize_focused(crate::layout::Direction::Horizontal, (dx / vp.size.w as f64) as f32);
            }
            if dy.abs() >= 1.0 {
                tree.resize_focused(crate::layout::Direction::Vertical, (dy / vp.size.h as f64) as f32);
            }
            self.relayout();
            return;
        }

        let dx = (self.pointer_location.x - drag.start_ptr.x).round() as i32;
        let dy = (self.pointer_location.y - drag.start_ptr.y).round() as i32;
        let ws = self.workspaces.active();
        let Some(entry) = ws.floating.iter_mut().find(|(w, _)| w == &drag.window) else { return };
        if drag.resize {
            entry.1.size.w = (drag.start_rect.size.w + dx).max(160);
            entry.1.size.h = (drag.start_rect.size.h + dy).max(120);
            if let Some(toplevel) = drag.window.toplevel() {
                toplevel.with_pending_state(|s| { s.size = Some(entry.1.size); });
                toplevel.send_pending_configure();
            }
        } else {
            entry.1.loc.x = drag.start_rect.loc.x + dx;
            entry.1.loc.y = drag.start_rect.loc.y + dy;
        }
        let loc = entry.1.loc;
        self.space.map_element(drag.window.clone(), loc, true);
        self.pending_redraw = true;
    }

    /// Click-to-focus: focus the window under the cursor (layer surfaces
    /// like the bar receive pointer events but never steal keyboard focus).
    pub fn focus_window_at_cursor(&mut self) {
        let pos = self.pointer_location;
        let Some(window) = self.space.element_under(pos).map(|(w, _)| w.clone()) else { return };
        let ws = self.workspaces.active();
        if ws.floating.iter().any(|(w, _)| w == &window) {
            ws.focus_floating = Some(window.clone());
        } else if ws.tree.contains(&window) {
            ws.focus_floating = None;
            ws.tree.focus_window(&window);
        }
        self.update_keyboard_focus();
    }

    /// Focus a window by its compositor id (IPC). Switches workspace if needed.
    pub fn focus_window_by_id(&mut self, id: u32) -> bool {
        let target = self.workspaces.all_windows().into_iter()
            .find(|w| window_id(w) == id);
        let Some(window) = target else { return false };
        if let Some(ws_id) = self.workspaces.find_workspace(&window) {
            if ws_id != self.workspaces.active_id() {
                self.switch_workspace(ws_id);
            }
        }
        let ws = self.workspaces.active();
        if ws.floating.iter().any(|(w, _)| w == &window) {
            ws.focus_floating = Some(window.clone());
        } else {
            ws.focus_floating = None;
            ws.tree.focus_window(&window);
        }
        self.update_keyboard_focus();
        true
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
                if let Some(w) = self.focused_window() {
                    if let Some(t) = w.toplevel() {
                        t.send_close();
                    }
                }
            }
            FocusNext => {
                self.workspaces.active().focus_floating = None;
                self.workspaces.active().tree.focus_next();
                self.update_keyboard_focus();
            }
            FocusPrev => {
                self.workspaces.active().focus_floating = None;
                self.workspaces.active().tree.focus_prev();
                self.update_keyboard_focus();
            }
            FocusDir(d)         => self.focus_dir(d),
            MoveDir(d)          => self.move_dir(d),
            ResizeDir(d)        => self.resize_dir(d),
            SetNextSplit(dir)   => { self.workspaces.active().tree.next_split_override = Some(dir); }
            Workspace(n)        => self.switch_workspace(n),
            MoveToWorkspace(n)  => self.move_focused_to_workspace(n),
            ToggleFloating      => self.toggle_floating(),
            ToggleFullscreen    => self.toggle_fullscreen(),
            ToggleOverview      => self.toggle_overview(),
            ToggleBlur          => {
                self.config.theme.blur = !self.config.theme.blur;
                tracing::info!(blur = self.config.theme.blur, "toggle blur");
                self.pending_redraw = true;
            }
            CycleOpacity        => {
                if let Some(w) = self.focused_window() {
                    // Opaque → 85% → 65% → opaque. Starts from the theme
                    // default so a global `opacity` setting cycles sensibly.
                    let cur = crate::state::window_opacity(&w, self.config.theme.opacity);
                    let next = if cur > 0.95 { 0.85 } else if cur > 0.75 { 0.65 } else { 1.0 };
                    crate::state::set_window_opacity(&w, next);
                    self.pending_redraw = true;
                }
            }
            Lock                => self.lock_session(),
            Quit => { self.quit_requested = true; return true; }
        }
        false
    }

    /// Output area minus layer-shell exclusive zones (the bar), minus the
    /// outer gap. Tiles are later shrunk by GAP_IN/2 per side, so windows
    /// sit GAP_IN apart and GAP_OUT from the screen edge.
    fn tiling_viewport(&self) -> Option<Rectangle<i32, Logical>> {
        let output = self.space.outputs().next()?.clone();
        let geometry = self.space.output_geometry(&output)?;
        let layer_map = layer_map_for_output(&output);
        let non_exclusive = layer_map.non_exclusive_zone();
        drop(layer_map);
        let mut viewport = geometry;
        viewport.loc  += non_exclusive.loc;
        viewport.size  = non_exclusive.size;
        let margin = self.config.theme.margin - self.config.theme.gap / 2;
        viewport.loc.x  += margin;
        viewport.loc.y  += margin;
        viewport.size.w  = (viewport.size.w - margin * 2).max(1);
        viewport.size.h  = (viewport.size.h - margin * 2).max(1);
        Some(viewport)
    }

    /// Recompute every mapped window's rectangle from the active workspace:
    /// tiled tree → viewport splits, floating → stored rects (raised),
    /// fullscreen → whole output, on top of everything.
    pub fn relayout(&mut self) {
        self.workspaces.prune_dead();
        let Some(output) = self.space.outputs().next().cloned() else { return };
        let Some(geometry) = self.space.output_geometry(&output) else { return };
        let Some(viewport) = self.tiling_viewport() else { return };

        // Safety net: unmap anything that isn't on the active workspace.
        let visible = self.workspaces.active_ref().windows();
        let stray: Vec<Window> = self.space.elements()
            .filter(|w| !visible.contains(*w))
            .cloned()
            .collect();
        for w in stray { self.space.unmap_elem(&w); }

        let gap = self.config.theme.gap;
        let layouts = self.workspaces.active_ref().tree.layout(viewport);
        for (window, mut rect) in layouts {
            // Inner gap: half per side so neighbors end up `gap` apart.
            rect.loc.x  += gap / 2;
            rect.loc.y  += gap / 2;
            rect.size.w  = (rect.size.w - gap).max(1);
            rect.size.h  = (rect.size.h - gap).max(1);
            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|s| { s.size = Some(rect.size); });
                toplevel.send_configure();
            }
            // Tile moved or resized → morph it over (only when already mapped).
            self.push_geo_anim(&window, rect);
            self.space.map_element(window, rect.loc, false);
        }

        // Floating layer sits above tiled windows.
        let floating = self.workspaces.active_ref().floating.clone();
        for (window, rect) in floating {
            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|s| { s.size = Some(rect.size); });
                toplevel.send_pending_configure();
            }
            self.push_geo_anim(&window, rect);
            self.space.map_element(window.clone(), rect.loc, false);
            self.space.raise_element(&window, false);
        }

        // Fullscreen override covers the whole output (incl. the bar zone).
        let fullscreen = self.workspaces.active_ref().fullscreen.clone();
        if let Some(window) = fullscreen.filter(|w| w.alive()) {
            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|s| { s.size = Some(geometry.size); });
                toplevel.send_configure();
            }
            self.push_geo_anim(&window, geometry);
            self.space.map_element(window.clone(), geometry.loc, false);
            self.space.raise_element(&window, true);
        }

        self.pending_redraw = true;
    }

    /// Toggle the overview grid. Entering queues a morph from every window's
    /// real geometry toward its grid cell; leaving morphs back. The grid is
    /// recomputed each frame from the same inputs, so both ends agree.
    pub fn toggle_overview(&mut self) {
        let now = std::time::Instant::now();
        tracing::info!(entering = !self.overview, "overview toggled");
        if !self.overview {
            if self.workspaces.all_windows().is_empty() { return; }
            let cells = self.overview_cells();
            self.overview = true;
            self.overview_t = now;
            self.drag = None;
            for (window, _) in cells {
                if let Some(geo) = self.space.element_geometry(&window) {
                    self.geo_anims.retain(|(w, _, _)| w != &window);
                    self.geo_anims.push((window, geo, now));
                }
            }
        } else {
            let cells = self.overview_cells();
            self.overview = false;
            self.overview_t = now;
            for (window, cell) in cells {
                self.geo_anims.retain(|(w, _, _)| w != &window);
                self.geo_anims.push((window, cell, now));
            }
        }
        self.pending_redraw = true;
        // Tell the bar (vendibar-pro) so it can show its overview chrome —
        // the spaces strip + hint overlay — in sync with the exposé.
        self.pending_ipc_events.push(crate::ipc::Event::Overview { active: self.overview });
    }

    /// Overview layout: one panel per workspace (non-empty ones plus the
    /// active one), aspect-matched miniatures of the output in a centered
    /// grid. Active-workspace windows keep their real spatial layout scaled
    /// into their panel (so the morph reads as one zoom); hidden workspaces
    /// get a small grid of thumbnails. Deterministic across frames — the
    /// renderer and click hit-testing both rely on it.
    pub fn overview_layout(&self) -> OverviewLayout {
        let mut out = OverviewLayout::default();
        let Some(output) = self.space.outputs().next() else { return out };
        let Some(geo) = self.space.output_geometry(output) else { return out };
        let active = self.workspaces.active_id();

        let panels: Vec<u32> = self.workspaces.iter()
            .filter(|ws| !ws.is_empty() || ws.id == active)
            .map(|ws| ws.id)
            .collect();
        let k = panels.len() as i32;
        if k == 0 { return out; }

        // Margins shrink when there are few panels — a 2-workspace overview
        // wants big panels, a 6-workspace one needs breathing room.
        let (mx_div, top_div, bot_div) = if k <= 3 { (26, 14, 22) } else { (14, 9, 14) };
        let margin_x = geo.size.w / mx_div;
        let top      = geo.size.h / top_div;
        let bottom   = geo.size.h / bot_div;
        let area = Rectangle::<i32, Logical>::new(
            (geo.loc.x + margin_x, geo.loc.y + top).into(),
            (geo.size.w - margin_x * 2, geo.size.h - top - bottom).into(),
        );
        let cols = (k as f64).sqrt().ceil() as i32;
        let rows = (k + cols - 1) / cols;
        let gap  = if k <= 3 { 28 } else { 36 };
        let slot_w = ((area.size.w - gap * (cols - 1)) / cols).max(1);
        let slot_h = ((area.size.h - gap * (rows - 1)) / rows).max(1);
        // Panels mirror the output's aspect so their content scales uniformly.
        let s = (slot_w as f64 / geo.size.w as f64).min(slot_h as f64 / geo.size.h as f64);
        let (pw, ph) = (((geo.size.w as f64 * s) as i32).max(1), ((geo.size.h as f64 * s) as i32).max(1));

        for (i, ws_id) in panels.iter().copied().enumerate() {
            let (row, col) = (i as i32 / cols, i as i32 % cols);
            // Center a short last row instead of leaving it ragged-left.
            let in_row = if row == rows - 1 { k - row * cols } else { cols };
            let row_w  = in_row * pw + (in_row - 1) * gap;
            let px = area.loc.x + (area.size.w - row_w) / 2 + col * (pw + gap);
            let py = area.loc.y + row * (slot_h + gap) + (slot_h - ph) / 2;
            let panel = Rectangle::<i32, Logical>::new((px, py).into(), (pw, ph).into());
            out.panels.push((ws_id, panel, ws_id == active));

            let Some(ws) = self.workspaces.iter().find(|w| w.id == ws_id) else { continue };
            if ws_id == active {
                // Mapped windows: scale their real geometry into the panel.
                for window in self.space.elements() {
                    let Some(wgeo) = self.space.element_geometry(window) else { continue };
                    let cell = Rectangle::<i32, Logical>::new(
                        (panel.loc.x + ((wgeo.loc.x - geo.loc.x) as f64 * s) as i32,
                         panel.loc.y + ((wgeo.loc.y - geo.loc.y) as f64 * s) as i32).into(),
                        (((wgeo.size.w as f64 * s) as i32).max(1),
                         ((wgeo.size.h as f64 * s) as i32).max(1)).into(),
                    );
                    out.cells.push((window.clone(), cell, ws_id));
                }
            } else {
                // Hidden windows have no live geometry — thumbnail grid.
                let windows = ws.windows();
                let n = windows.len() as i32;
                if n == 0 { continue; }
                let inset = (ph / 12).max(8);
                let inner = Rectangle::<i32, Logical>::new(
                    (panel.loc.x + inset, panel.loc.y + inset).into(),
                    ((pw - inset * 2).max(1), (ph - inset * 2).max(1)).into(),
                );
                let gcols = (n as f64).sqrt().ceil() as i32;
                let grows = (n + gcols - 1) / gcols;
                let ggap = 10;
                let cw = ((inner.size.w - ggap * (gcols - 1)) / gcols).max(1);
                let ch = ((inner.size.h - ggap * (grows - 1)) / grows).max(1);
                for (j, window) in windows.into_iter().enumerate() {
                    let size = window.geometry().size;
                    if size.w <= 0 || size.h <= 0 { continue; }
                    let (grow, gcol) = (j as i32 / gcols, j as i32 % gcols);
                    let in_row = if grow == grows - 1 { n - grow * gcols } else { gcols };
                    let row_w  = in_row * cw + (in_row - 1) * ggap;
                    let cx = inner.loc.x + (inner.size.w - row_w) / 2 + gcol * (cw + ggap);
                    let cy = inner.loc.y + grow * (ch + ggap);
                    let fs = (cw as f64 / size.w as f64).min(ch as f64 / size.h as f64).min(1.0);
                    let tw = ((size.w as f64 * fs) as i32).max(1);
                    let th = ((size.h as f64 * fs) as i32).max(1);
                    out.cells.push((window, Rectangle::new(
                        (cx + (cw - tw) / 2, cy + (ch - th) / 2).into(),
                        (tw, th).into(),
                    ), ws_id));
                }
            }
        }
        out
    }

    /// Flat (window, cell) list — the morph and hit-test view of the layout.
    pub fn overview_cells(&self) -> Vec<(Window, Rectangle<i32, Logical>)> {
        self.overview_layout().cells.into_iter().map(|(w, r, _)| (w, r)).collect()
    }

    /// Lock the session: the renderer switches to the lock screen and the
    /// keyboard filter routes everything into the password buffer.
    pub fn lock_session(&mut self) {
        if self.vlock { return; }
        tracing::info!("session locked");
        self.vlock = true;
        self.vlock_input.clear();
        self.vlock_fail = None;
        self.drag = None;
        self.overview = false;
        self.pending_redraw = true;
    }

    /// Try the buffered password against PAM (same stack as login). The
    /// buffer is consumed either way.
    pub fn lock_submit(&mut self) {
        let pass = std::mem::take(&mut self.vlock_input);
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| {
                users::get_current_username()
                    .map(|u| u.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
        let ok = (|| {
            let mut auth = pam::Authenticator::with_password("system-auth").ok()?;
            auth.get_handler().set_credentials(user.as_str(), pass.as_str());
            auth.authenticate().ok()
        })().is_some();
        if ok {
            tracing::info!("session unlocked");
            self.vlock = false;
            self.vlock_fail = None;
        } else {
            tracing::info!("unlock failed");
            self.vlock_fail = Some(std::time::Instant::now());
        }
        self.pending_redraw = true;
    }

    /// Focus + raise a specific window (overview click).
    pub fn focus_window(&mut self, window: &Window) {
        let ws = self.workspaces.active();
        if ws.floating.iter().any(|(w, _)| w == window) {
            ws.focus_floating = Some(window.clone());
        } else if ws.tree.contains(window) {
            ws.focus_floating = None;
            ws.tree.focus_window(window);
        }
        self.update_keyboard_focus();
    }

    /// Queue a layout morph from the window's current geometry to `target`
    /// (no-op when nothing changed or the window isn't mapped yet). During a
    /// Super+drag the window must track the pointer 1:1, so drags don't morph.
    fn push_geo_anim(&mut self, window: &Window, target: Rectangle<i32, Logical>) {
        // During any drag, geometry must track the pointer 1:1 — a tiled
        // resize relayouts every motion event and morphs would lag behind.
        if self.drag.is_some() { return; }
        let Some(old) = self.space.element_geometry(window) else { return };
        if old == target { return; }
        self.geo_anims.retain(|(w, _, _)| w != window);
        self.geo_anims.push((window.clone(), old, std::time::Instant::now()));
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
