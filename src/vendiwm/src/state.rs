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
        pointer::{CursorImageStatus, MotionEvent, ButtonEvent},
    },
    backend::input::ButtonState,
    utils::{IsAlive, Logical, Point, Rectangle, SERIAL_COUNTER, Serial},
    reexports::wayland_server::{
        Client, DisplayHandle, Resource,
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
    /// xdg-activation: apps hand each other focus (a link clicked in chat
    /// raises the browser; a notification's action raises its app).
    pub xdg_activation_state:  smithay::wayland::xdg_activation::XdgActivationState,
    pub idle_inhibitors:       std::collections::HashSet<WlSurface>,
    pub xdg_decoration_state:  smithay::wayland::shell::xdg::decoration::XdgDecorationState,
    pub viewporter_state:      ViewporterState,
    pub fractional_scale_manager_state: FractionalScaleManagerState,
    pub seat:                  Seat<Self>,
    /// Kept so actions can resolve a client's PID (force-kill) via
    /// `Client::get_credentials`.
    pub display_handle:        DisplayHandle,

    // ── XWayland ────────────────────────────────────────────────────────────
    // X11 apps (Discord/Electron — which choke on NVIDIA's native-Wayland gbm
    // scanout path — plus any other X11-only client) run via the Xserver. The
    // xwayland-shell global associates each X11 window with the wl_surface it
    // renders to; `xwm` is the X11 window manager once XWayland is up; `xdisplay`
    // is the `:N` exported as $DISPLAY for spawned clients. Everything dispatches
    // on State (the calloop event loop runs on State so `X11Wm::start_wm` gets
    // its required `LoopHandle<'static, State>`).
    #[cfg(feature = "xwayland")]
    pub xwayland_shell_state:  smithay::wayland::xwayland_shell::XWaylandShellState,
    #[cfg(feature = "xwayland")]
    pub xwm:                   Option<smithay::xwayland::X11Wm>,
    #[cfg(feature = "xwayland")]
    pub xdisplay:              Option<u32>,

    // udev/DRM runtime (real session backend). Lives on State because the
    // event loop dispatches on State; None under the nested winit dev backend.
    #[cfg(feature = "udev")]
    pub udev:                  Option<crate::backends::udev::UdevData>,

    // ext-session-lock: a client (swaylock) asked to lock. We confirm only
    // after the first locked frame has actually been queued — until then
    // `lock_pending` holds the unconfirmed locker. While locked the renderer
    // shows ONLY the lock surface (black if the client hasn't mapped one).
    pub lock_pending:          Option<SessionLocker>,
    pub locked:                bool,
    /// One lock surface per monitor (output name, surface) — the locker
    /// sizes each to its own monitor, so each is drawn only there.
    pub lock_surfaces:         Vec<(String, LockSurface)>,

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

    // Single-finger touchscreen → pointer emulation. The first finger down
    // drives a synthetic mouse (tap = left click, drag = left-button drag,
    // long-press = right click, Super-held = window move). See TouchEmul.
    pub touch:                 Option<TouchEmul>,
    // All fingers currently on the touchscreen, and any multi-finger gesture
    // accumulating across them (3-finger horizontal swipe = workspace).
    pub touch_points:          HashMap<smithay::backend::input::TouchSlot, Point<f64, Logical>>,
    pub touch_gesture:         Option<TouchGesture>,

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
    pub last_zone:             Vec<(String, Rectangle<i32, Logical>)>,

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
    // Which monitor the workspace-switch animation plays on.
    pub ws_anim_output:        String,

    // Layout-morph animations: (window, previous geometry, started-at). The
    // render loop interpolates location AND size, so tile moves, split
    // resizes, and fullscreen toggles all glide instead of snapping.
    pub geo_anims:             Vec<(Window, Rectangle<i32, Logical>, std::time::Instant)>,

    // First-frame timestamp, set lazily by the render loop. Boot used to slam
    // the composed desktop on screen the instant the compositor came up —
    // straight from the LUKS prompt to a full wallpaper with no transition at
    // all. Everything fades up from black over this instead.
    pub startup_t:             Option<std::time::Instant>,

    // Fullscreen enter/exit: (started-at, entering, window). Drives the bar's
    // fade and the window's corner-radius/border fade in lockstep with its
    // geo glide, instead of the bar and corners snapping instantly while the
    // window is still animating.
    pub fullscreen_anim:       Option<(std::time::Instant, bool, Window)>,

    // Windows that just closed: (protocol id, last on-screen geometry). The
    // backend pairs these with its per-frame texture stash and plays a
    // fade-and-shrink. The surface is gone by now — only the texture lives.
    pub closing:               Vec<(u32, Rectangle<i32, Logical>)>,

    // Last rendered geometry per window (protocol id) — the close ghost's
    // fallback when a client unmaps before destroying (Firefox does), at
    // which point the space no longer knows where the window was.
    pub last_geos:             HashMap<u32, Rectangle<i32, Logical>>,
    // Intended tile rects from the last relayout (per window id). During an
    // interactive seam-resize the committed buffer lags the layout, so the
    // renderer morph-scales the buffer to THIS rect to avoid edge overdraw.
    pub tile_geos:             HashMap<u32, Rectangle<i32, Logical>>,
    // Live drop preview while drag-rearranging a tile (Some during the drag).
    pub drop_preview:          Option<DropPreview>,
    // Compositor-drawn chrome: tab strips, the stage shelf, pinned windows.
    pub chrome:                crate::chrome::ChromeState,
    // A touch press on chrome turned into a tear-off drag; the finger's
    // motion/lift drive that drag instead of the emulated pointer.
    pub touch_chrome_drag:     bool,

    // Compiled keybinds + future settings, loaded at startup from KDL.
    pub config:                Config,

    // Current pointer position in compositor logical coordinates.
    pub pointer_location:      Point<f64, Logical>,

    // What the pointer should look like, as last requested by the focused client
    // (cursor-shape-v1 named shape, a client-drawn surface, or hidden). The
    // backend renders accordingly; defaults to the themed arrow.
    pub cursor_status:         CursorImageStatus,

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
    // Monitors (by name) with new client content — a targeted pending_redraw.
    pub dirty_outputs:          Vec<String>,

    // Set by ReloadConfig when a monitor's mode/refresh may have changed. The
    // udev backend reads + clears it each tick and reprograms the affected DRM
    // surface live (DrmCompositor::use_mode) — so Hz/resolution hot-reload with
    // just a brief blackout instead of needing a session restart.
    pub pending_output_modes:   bool,

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
    // Last time a tile-resize actually relayout+reconfigured. Fast mouse motion
    // is coalesced to a frame budget so clients aren't flooded with stale
    // configures (which makes them visibly trail the cursor).
    pub last_apply:  std::time::Instant,
    // The window was tiled when the grab began (detached in place to follow the
    // cursor) — on release it re-tiles at the drop target instead of floating.
    pub from_tile:   bool,
    // Smoothed pointer velocity (logical px/s) over the drag, when it was last
    // sampled, and the pointer position at that sample. Powers throw-to-snap: a
    // quick flick toward a neighbor lands the tile there even if the cursor
    // stops short of it. (start_ptr isn't re-anchored for a from_tile drag, so
    // per-frame delta must come from last_ptr, not start_ptr.)
    pub vel:         (f64, f64),
    pub last_motion: std::time::Instant,
    pub last_ptr:    Point<f64, Logical>,
}

/// Live drop preview while dragging a detached tile. The hovered tile (`target`)
/// is split in half: it live-resizes to `t_rect`, and a ghost placeholder (gray
/// border + frost, no content) fills `ghost`. `dir`/`before` capture how to make
/// the same split for real on drop.
#[derive(Clone)]
pub struct DropPreview {
    pub target: Window,
    pub t_rect: Rectangle<i32, Logical>,
    pub ghost:  Rectangle<i32, Logical>,
    pub dir:    crate::layout::Direction,
    pub before: bool,   // the dragged window goes before the target (left/top)
    // Workspace-EDGE drop: the window takes a full column/row at the edge and is
    // inserted at the tree ROOT (the whole layout reflows into the other half),
    // rather than splitting a single hovered tile. `target`/`t_rect` are unused.
    pub root_edge: bool,
    // Drop onto a tab strip or a tile's middle: the dragged window joins the
    // target's tab group instead of splitting it. `ghost` = the whole tile.
    pub join: bool,
}

/// Single-finger touch→pointer emulation. vendiOS targets laptops with a
/// touchscreen (keyboard always present), so touch acts like the mouse rather
/// than delivering native wl_touch: tap = left click, a drag past a small
/// threshold = left-button drag (scroll/select), a stationary hold = right
/// click (context menus), and Super held at touch-down = window move (the same
/// grab as Super+LeftDrag with a mouse). Only the first finger emulates; extra
/// fingers are ignored here (multi-finger gestures are handled separately).
pub struct TouchEmul {
    /// The libinput slot of the finger we're tracking (the first one down).
    pub slot:        smithay::backend::input::TouchSlot,
    pub down_pos:    Point<f64, Logical>,
    pub down_time:   u32,
    pub down_instant: std::time::Instant,
    pub cur_pos:     Point<f64, Logical>,
    pub phase:       TouchPhase,
    /// Touch started at the very top screen edge — a downward drag from here
    /// pulls the control center instead of acting as a pointer drag.
    pub from_edge:   bool,
}

/// Multi-finger touchscreen gesture in progress (2+ fingers). Accumulates the
/// summed finger travel; a 3-finger horizontal swipe switches workspaces.
#[derive(Clone, Copy)]
pub struct TouchGesture {
    pub fingers: usize,
    pub dx:      f64,
    pub dy:      f64,
    pub fired:   bool,
}

#[derive(PartialEq, Clone, Copy)]
pub enum TouchPhase {
    /// Finger down, not yet moved far or held long — could become tap / drag /
    /// long-press. No button has been sent to the client yet.
    Pending,
    /// Moved past the threshold: left button is held and we're dragging.
    Dragging,
    /// Super was held at touch-down: driving a window-move grab (`self.drag`).
    WindowMove,
    /// A gesture already resolved (long-press fired); swallow until lift.
    Consumed,
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
        // The XWayland connection is a wayland client too, but smithay attaches
        // its own `XWaylandClientData` (not our `ClientState`). Without this
        // branch, the first surface XWayland commits panics here on unwrap().
        #[cfg(feature = "xwayland")]
        if let Some(xdata) = client.get_data::<smithay::xwayland::XWaylandClientData>() {
            return &xdata.compositor_state;
        }
        &client.get_data::<ClientState>().unwrap().compositor_state
    }
    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);

        // Any surface commit (window, popup, layer-shell bar/notification,
        // cursor, subsurface) is new content to present. Previously ONLY the
        // screensaver's special case below armed pending_redraw — every other
        // client's updates (a blinking cursor, video, the bar's clock, typing)
        // relied entirely on the VBlank handler unconditionally re-rendering
        // forever to ever be picked up, which is why VBlank could never be
        // gated on pending_redraw without freezing normal rendering. This is
        // what actually lets that gate exist (see the VBlank handler).
        //
        // Multi-monitor: only the monitor(s) the surface is on owe a frame —
        // a terminal spinner on the laptop mustn't re-composite a 1440p@144
        // screen. Unknown surfaces (cursor, popups, not-yet-mapped) redraw all.
        match self.surface_outputs(surface) {
            Some(outs) if !outs.is_empty() => {
                for o in outs {
                    if !self.dirty_outputs.contains(&o) { self.dirty_outputs.push(o); }
                }
            }
            _ => self.pending_redraw = true,
        }

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
            if self.is_phone(&window) { self.phone_commit(&window); }

            // The screensaver lives outside the tiling tree; pending_redraw is
            // now armed unconditionally above for every commit, so this block
            // only needs its slide-in timing, not a second pending_redraw set.
            if self.screensaver.as_ref() == Some(&window) {
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
                    use smithay::wayland::shell::xdg::SurfaceCachedState;
                    let app_id = with_states(toplevel.wl_surface(), |states| {
                        states.data_map.get::<XdgToplevelSurfaceData>()
                            .and_then(|d| d.lock().ok().and_then(|a| a.app_id.clone()))
                            .unwrap_or_default()
                    });
                    let (min, max) = with_states(toplevel.wl_surface(), |states| {
                        let mut cs = states.cached_state.get::<SurfaceCachedState>();
                        let c = cs.current();
                        (c.min_size, c.max_size)
                    });
                    // Fixed-size windows (splash screens, Discord's updater/loader)
                    // and dialogs with a parent don't belong in the tiling grid:
                    // stretching them to a tile leaves them blank/broken (Discord
                    // never finishes loading). Float them at their intended size.
                    let fixed_size = min.w > 0 && min.h > 0 && min == max;
                    let has_parent = toplevel.parent().is_some();
                    // User window rules (config `rules { rule … }`) — title is
                    // already cached in window_titles by the block above.
                    let rule_title = self.window_titles.get(&id).cloned().unwrap_or_default();
                    let process = self.window_process(&window);
                    let eff = self.config.match_window(&app_id, &rule_title, &process);
                    if !app_id.is_empty() || has_parent || fixed_size || !eff.is_empty() {
                        self.rule_checked.insert(id);
                        // Float: an explicit rule wins; otherwise the built-in
                        // dialog / fixed-size heuristic decides.
                        let float = eff.float.unwrap_or(
                            app_id == "vendi-float" || has_parent || fixed_size);
                        if float { self.float_window(&window); }
                        if let Some(op) = eff.opacity {
                            crate::state::set_window_opacity(&window, op);
                        }
                        if eff.fullscreen == Some(true) {
                            self.set_fullscreen(&window, true);
                        }
                        if eff.phone == Some(true) {
                            self.make_phone(&window);
                        }
                        if let Some(ws) = eff.workspace {
                            if ws != self.workspaces.active_id() {
                                self.workspaces.move_window_to(&window, ws);
                                self.space.unmap_elem(&window);
                                self.relayout();
                                self.update_keyboard_focus();
                                self.emit_workspaces();
                            }
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
            // (per monitor: a single shared value flip-flopped between two
            // monitors' zones and re-tiled everything on every layer commit)
            let oname = output.name();
            let prev = self.last_zone.iter().find(|(n, _)| *n == oname).map(|(_, z)| *z);
            if !initial_configure_sent || prev != Some(zone) {
                self.last_zone.retain(|(n, _)| *n != oname);
                self.last_zone.push((oname, zone));
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
                    // Compare by wl_surface (current_focus is now KbFocus).
                    if kb.current_focus().and_then(|f| f.wl_surface()) != desired {
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
        // Start it on the focused monitor (global 0,0 may be on no monitor at
        // all with a custom arrangement); relayout then puts it in its tile.
        let start = self.tiling_viewport().map(|v| v.loc).unwrap_or_default();
        self.space.map_element(window, start, true);
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
    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        // GTK (nautilus, file dialogs, every menu/dropdown) uses reactive
        // positioning and repositions its popup right after mapping. The
        // protocol requires us to apply the new positioner, then reply with
        // `repositioned` + a fresh configure. Ignoring it (the old no-op) left
        // GTK waiting forever — the menu never showed and input stayed grabbed.
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
        if let Err(e) = surface.send_configure() {
            tracing::warn!(?e, "popup reposition configure failed");
        }
    }
}

impl SelectionHandler for State {
    type SelectionUserData = ();

    // A Wayland client took the selection — push the offered mime types into the
    // X11Wm so X11 apps (Discord/Electron under XWayland) can paste from Wayland.
    // Without this, copy-in-terminal → paste-in-X11 silently does nothing.
    #[allow(unused_variables)]
    fn new_selection(
        &mut self,
        ty: smithay::wayland::selection::SelectionTarget,
        source: Option<smithay::wayland::selection::SelectionSource>,
        _seat: Seat<Self>,
    ) {
        #[cfg(feature = "xwayland")]
        if let Some(xwm) = self.xwm.as_mut() {
            if let Err(err) = xwm.new_selection(ty, source.map(|s| s.mime_types())) {
                tracing::warn!(?err, ?ty, "failed to push selection to Xwayland");
            }
        }
    }

    // A Wayland client wants to read a server-set selection. The only server-set
    // selections we create are the X11-owned ones bridged in XwmHandler, so read
    // them back out of the X server and write to the client's fd. Without this,
    // copy-in-X11 → paste-in-Wayland (e.g. into the terminal) yields nothing.
    #[allow(unused_variables)]
    fn send_selection(
        &mut self,
        ty: smithay::wayland::selection::SelectionTarget,
        mime_type: String,
        fd: std::os::unix::io::OwnedFd,
        _seat: Seat<Self>,
        _user_data: &(),
    ) {
        #[cfg(feature = "xwayland")]
        if let Some(xwm) = self.xwm.as_mut() {
            if let Err(err) = xwm.send_selection(ty, mime_type, fd) {
                tracing::warn!(?err, "failed to send Xwayland selection to Wayland client");
            }
        }
    }
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
        self.lock_surfaces.clear();
        self.update_keyboard_focus();
        self.pending_redraw = true;
    }
    fn new_surface(&mut self, surface: LockSurface, output: wl_output::WlOutput) {
        let out = Output::from_resource(&output);
        let size = out.as_ref()
            .and_then(|o| self.space.output_geometry(o))
            .map(|g| g.size)
            .unwrap_or_else(|| (1920, 1080).into());
        surface.with_pending_state(|s| {
            s.size = Some((size.w as u32, size.h as u32).into());
        });
        surface.send_configure();
        let name = out.map(|o| o.name()).unwrap_or_default();
        self.lock_surfaces.retain(|(n, s)| n != &name && s.alive());
        self.lock_surfaces.push((name, surface));
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
            // the lock surface of the monitor under the pointer, at that
            // monitor's origin
            let out = self.space.output_under(pos).next().cloned()?;
            let origin = self.space.output_geometry(&out)?.loc.to_f64();
            return self.lock_surface_on(&out.name()).map(|l| (l.wl_surface().clone(), origin));
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
        // The monitor under the pointer — its own bar/layers take the input
        // (every monitor has a bar; only the first one's used to be clickable).
        let output = self.space.output_under(pos).next().cloned()
            .or_else(|| self.space.outputs().next().cloned())?;
        let out_geo = self.space.output_geometry(&output)?;
        // A fullscreen window covers the Top layer: the renderer already drops
        // the bar from the frame, so it must not take pointer input either.
        // Otherwise clicks along the bar's strip land on a surface that isn't
        // on screen and never reach the fullscreen app underneath. Overlay
        // stays live either way, per the wlr-layer-shell spec — that's where a
        // lock screen lives. Kept as the same test the renderer uses, so input
        // and rendering can't drift apart.
        let fullscreen_active = self.workspaces.shown_on(&output.name())
            .and_then(|id| self.workspaces.get(id))
            .is_some_and(|ws| ws.fullscreen.is_some());
        {
            let map = layer_map_for_output(&output);
            let upper: &[Layer] = if fullscreen_active {
                &[Layer::Overlay]
            } else {
                &[Layer::Overlay, Layer::Top]
            };
            for l in upper.iter().copied() {
                if let Some(layer) = map.layer_under(l, pos - out_geo.loc.to_f64()) {
                    if let Some(hit) = layer_hit(&map, layer, out_geo.loc, pos) { return Some(hit); }
                }
            }
        }
        if let Some((window, loc)) = self.window_under(pos) {
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
        // any monitor's launcher / lock screen
        for output in self.space.outputs() {
            let map = layer_map_for_output(output);
            let layer = map.layers().rev().find(|l| {
                if !matches!(l.layer(), Layer::Top | Layer::Overlay) { return false; }
                with_states(l.wl_surface(), |states| {
                    states.cached_state.get::<LayerSurfaceCachedState>().current()
                        .keyboard_interactivity != KeyboardInteractivity::None
                })
            });
            if let Some(layer) = layer { return Some(layer.wl_surface().clone()); }
        }
        None
    }

    /// Push keyboard focus + xdg Activated state to whatever the active
    /// workspace says is focused, raise it, and tell the bar.
    pub fn update_keyboard_focus(&mut self) {
        // A lock surface owns the keyboard unconditionally while locked.
        if self.is_locked() {
            let focused = self.focused_output().map(|o| o.name()).unwrap_or_default();
            let surf = self.lock_surface_on(&focused)
                .or_else(|| self.lock_surfaces.first().map(|(_, s)| s))
                .map(|l| KbFocus::Wl(l.wl_surface().clone()));
            if let Some(kb) = self.seat.get_keyboard() {
                kb.set_focus(self, surf, SERIAL_COUNTER.next_serial());
            }
            self.pending_redraw = true;
            return;
        }
        if let Some(surf) = self.exclusive_layer_surface() {
            if let Some(kb) = self.seat.get_keyboard() {
                kb.set_focus(self, Some(KbFocus::Wl(surf)), SERIAL_COUNTER.next_serial());
            }
            self.pending_redraw = true;
            return;
        }
        let focused = self.focused_window().filter(|w| w.alive());
        // Build the right focus target: X11 windows MUST go through KbFocus::X11
        // so smithay sends WM_TAKE_FOCUS / sets X input focus per their input
        // model (else games never get the keyboard). Wayland windows use ::Wl.
        let kfocus: Option<KbFocus> = focused.as_ref().and_then(|w| {
            #[cfg(feature = "xwayland")]
            if let Some(x) = w.x11_surface() { return Some(KbFocus::X11(x.clone())); }
            w.wl_surface().map(|s| KbFocus::Wl(s.into_owned()))
        });
        if let Some(kb) = self.seat.get_keyboard() {
            kb.set_focus(self, kfocus, SERIAL_COUNTER.next_serial());
        }
        // Activated ring: set on the focused window, clear everywhere else.
        for window in self.workspaces.all_windows() {
            let active = Some(&window) == focused.as_ref();
            // X11 windows have no xdg toplevel — `set_activated` is what makes
            // smithay set the X input focus / send WM_TAKE_FOCUS. Without it,
            // "Globally Active" X11 apps (most games, e.g. Geometry Dash under
            // Proton) never grab the keyboard even when wl-focused.
            #[cfg(feature = "xwayland")]
            if let Some(x11) = window.x11_surface() {
                let _ = x11.set_activated(active);
            }
            let Some(toplevel) = window.toplevel() else { continue };
            toplevel.with_pending_state(|s| {
                if active { s.states.set(xdg_toplevel::State::Activated); }
                else      { s.states.unset(xdg_toplevel::State::Activated); }
            });
            toplevel.send_pending_configure();
        }
        if let Some(window) = &focused {
            self.space.raise_element(window, true);
            // pinned windows + phone mirrors stay above whatever has focus
            self.raise_overlays();
            // Also raise it in the X11 stack so the focused game/app is on top.
            #[cfg(feature = "xwayland")]
            if let Some(x11) = window.x11_surface() {
                if let Some(xwm) = self.xwm.as_mut() { let _ = xwm.raise_window(x11); }
            }
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
                .map(|(id, windows, output, visible)| crate::ipc::WorkspaceInfo { id, focused: id == active, windows, output, visible })
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
        // That desk is already on the other monitor: go there instead of
        // stealing it (sway/i3 behaviour) — focus + pointer jump across.
        if let Some(out) = self.workspaces.output_showing(id).map(str::to_string) {
            self.workspaces.switch_to(id);
            self.warp_to_output(&out);
            self.update_keyboard_focus();
            self.emit_workspaces();
            return;
        }
        let dir = if id > self.workspaces.active_id() { 1 } else { -1 };
        let pins = self.unpin_for_switch();
        let hidden = self.workspaces.switch_to(id);
        for w in hidden { self.space.unmap_elem(&w); }
        self.repin(pins);
        self.ws_anim = Some((std::time::Instant::now(), dir));
        // only the monitor that switched slides
        self.ws_anim_output = self.workspaces.focused_output().to_string();
        self.relayout();
        self.update_keyboard_focus();
        self.emit_workspaces();
    }

    /// Send the focused window to workspace `id` (it lands tiled there).
    pub fn move_focused_to_workspace(&mut self, id: u32) {
        let Some(window) = self.focused_window() else { return };
        self.workspaces.move_window_to(&window, id);
        // off screen → hide it; on screen on either monitor → relayout maps it
        if !self.workspaces.is_shown(id) {
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

    /// Re-center the focused floating window on its output, keeping its size.
    /// No-op for tiled windows.
    pub fn center_floating(&mut self) {
        let Some(window) = self.focused_window() else { return };
        let Some(vp) = self.tiling_viewport() else { return };
        let ws = self.workspaces.active();
        if let Some((_, rect)) = ws.floating.iter_mut().find(|(w, _)| w == &window) {
            rect.loc = (
                vp.loc.x + (vp.size.w - rect.size.w) / 2,
                vp.loc.y + (vp.size.h - rect.size.h) / 2,
            ).into();
            self.relayout();
        }
    }

    /// Stash the focused window onto the stage shelf (super+shift+minus). It
    /// flies to a live card on the left; click the card to bring it back.
    pub fn move_to_scratchpad(&mut self) {
        let Some(w) = self.focused_window() else { return };
        self.stash(&w, 0);
    }

    /// super+minus: show / hide the stage shelf.
    pub fn toggle_scratchpad(&mut self) {
        self.toggle_shelf();
    }

    /// Tabbed groups (super+g): if the focused tiled window is in a stack, cycle
    /// to the next tab; otherwise group it with the next tiled window into a new
    /// stack (they share one tile under a tab strip — see chrome.rs).
    pub fn group_or_cycle(&mut self) {
        let Some(f) = self.focused_window() else { return };
        if !self.workspaces.active_ref().tree.contains(&f) { return; } // tiled only

        // Already grouped → advance to the next member.
        if let Some(si) = self.workspaces.active_ref().stack_of(&f) {
            let (old, new) = {
                let ws = self.workspaces.active();
                let s = &mut ws.stacks[si];
                if s.members.len() < 2 { return; }
                let old = s.members[s.active].clone();
                s.active = (s.active + 1) % s.members.len();
                let new = s.members[s.active].clone();
                (old, new)
            };
            let ws = self.workspaces.active();
            ws.tree.replace_window(&old, &new);
            ws.tree.focus_window(&new);
            self.space.unmap_elem(&old);
            self.open_anims.retain(|(x, _)| x != &new);
            self.open_anims.push((new, None));
            self.relayout();
            self.update_keyboard_focus();
            self.emit_workspaces();
            return;
        }

        // Not grouped → stack with the next tiled window that isn't already in
        // a stack (so we never list one window in two stacks).
        let wins = self.workspaces.active_ref().tree.windows();
        if wins.len() < 2 { return; }
        let target = {
            let ws = self.workspaces.active_ref();
            let fi = wins.iter().position(|w| w == &f).unwrap_or(0);
            let n = wins.len();
            let mut t = None;
            for k in 1..n {
                let cand = &wins[(fi + k) % n];
                if cand != &f && ws.stack_of(cand).is_none() { t = Some(cand.clone()); break; }
            }
            t
        };
        let Some(target) = target else { return; };
        let ws = self.workspaces.active();
        ws.tree.remove(&target);
        ws.stacks.push(crate::workspaces::Stack {
            members: vec![f.clone(), target.clone()], active: 0,
        });
        ws.tree.focus_window(&f);
        self.space.unmap_elem(&target);
        self.relayout();
        self.update_keyboard_focus();
        self.emit_workspaces();
    }

    /// Ungroup (super+shift+g): dissolve the focused window's stack — the hidden
    /// members tile back in, the active one stays put.
    pub fn ungroup(&mut self) {
        let Some(f) = self.focused_window() else { return };
        let Some(si) = self.workspaces.active_ref().stack_of(&f) else { return };
        let ws = self.workspaces.active();
        let stack = ws.stacks.remove(si);
        let active = stack.members[stack.active].clone();
        for m in stack.members {
            if m == active { continue; }
            ws.tree.insert(m);
        }
        ws.tree.focus_window(&active);
        self.relayout();
        self.update_keyboard_focus();
        self.emit_workspaces();
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
        self.fullscreen_anim = Some((std::time::Instant::now(), on, window.clone()));
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

    /// The nearest other monitor in a screen direction from the focused one.
    pub fn output_in_dir(&self, dir: crate::input::Dir) -> Option<Output> {
        use crate::input::Dir;
        let cur = self.focused_output()?;
        let g = self.space.output_geometry(&cur)?;
        let c = (g.loc.x + g.size.w / 2, g.loc.y + g.size.h / 2);
        self.space.outputs()
            .filter(|o| **o != cur)
            .filter_map(|o| {
                let og = self.space.output_geometry(o)?;
                let oc = (og.loc.x + og.size.w / 2, og.loc.y + og.size.h / 2);
                let (dx, dy) = ((oc.0 - c.0) as i64, (oc.1 - c.1) as i64);
                let ok = match dir {
                    Dir::Left  => dx < 0 && dx.abs() >= dy.abs(),
                    Dir::Right => dx > 0 && dx.abs() >= dy.abs(),
                    Dir::Up    => dy < 0 && dy.abs() >= dx.abs(),
                    Dir::Down  => dy > 0 && dy.abs() >= dx.abs(),
                };
                ok.then_some((dx * dx + dy * dy, o.clone()))
            })
            .min_by_key(|(d, _)| *d)
            .map(|(_, o)| o)
    }

    /// Jump focus (and the pointer) to the monitor in `dir`.
    pub fn focus_output_dir(&mut self, dir: crate::input::Dir) {
        let Some(o) = self.output_in_dir(dir) else { return };
        let name = o.name();
        self.warp_to_output(&name);
        if self.workspaces.focus_output(&name) {
            self.update_keyboard_focus();
            self.emit_workspaces();
        }
    }

    /// Send the focused window to the desk on the monitor in `dir`; you and
    /// the pointer follow it there.
    pub fn move_to_output_dir(&mut self, dir: crate::input::Dir) {
        let Some(w) = self.focused_window() else { return };
        let Some(o) = self.output_in_dir(dir) else { return };
        let name = o.name();
        let Some(id) = self.workspaces.shown_on(&name) else { return };
        if self.chrome.pinned.contains(&w) { return; }
        self.detach_group_member(&w);
        self.workspaces.move_window_to(&w, id);
        self.warp_to_output(&name);
        self.workspaces.focus_output(&name);
        self.relayout();
        self.focus_window(&w);
        self.emit_workspaces();
    }

    pub fn focus_dir(&mut self, dir: crate::input::Dir) {
        // nothing further this way on this monitor → continue onto the next one
        let Some(target) = self.window_in_dir(dir) else { return self.focus_output_dir(dir) };
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
        // at the edge of this monitor → carry the window onto the next one
        let Some(target) = self.window_in_dir(dir) else { return self.move_to_output_dir(dir) };
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

        // Tiled right-drag / seam-drag: trade split ratios with the neighbors,
        // KDE-style. Coalesce to a frame budget — applying on every motion event
        // floods clients with stale configures so they trail the cursor.
        if drag.tile_resize {
            if drag.last_apply.elapsed() < std::time::Duration::from_millis(8) {
                return;   // accumulate the delta; don't re-anchor start_ptr
            }
            let Some(vp) = self.tiling_viewport() else { return };
            let dx = self.pointer_location.x - drag.start_ptr.x;
            let dy = self.pointer_location.y - drag.start_ptr.y;
            if let Some(d) = self.drag.as_mut() {
                d.start_ptr = self.pointer_location;
                d.last_apply = std::time::Instant::now();
            }
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

        // Live drop preview: when rearranging a tile — or aero-snapping a floating
        // window to a workspace edge — reflow the layout to show where it'll land
        // (only relayout when the target/split actually changes). Skip resizes.
        if !drag.resize {
            // Smooth pointer velocity (EMA) for throw-to-snap on release. Use
            // the per-frame delta (last_ptr → now); start_ptr isn't re-anchored.
            let ptr = self.pointer_location;
            if let Some(d) = self.drag.as_mut() {
                let dt = d.last_motion.elapsed().as_secs_f64().max(1.0 / 1000.0);
                if dt < 0.1 {
                    let inst = ((ptr.x - d.last_ptr.x) / dt, (ptr.y - d.last_ptr.y) / dt);
                    d.vel.0 = d.vel.0 * 0.6 + inst.0 * 0.4;
                    d.vel.1 = d.vel.1 * 0.6 + inst.1 * 0.4;
                } else {
                    d.vel = (0.0, 0.0); // long idle pause → not a throw
                }
                d.last_motion = std::time::Instant::now();
                d.last_ptr = ptr;
            }
            // Over the stage shelf: no layout preview; the shelf lights up.
            let over_shelf = self.pointer_over_shelf() && !self.chrome.pinned.contains(&drag.window);
            if over_shelf != self.chrome.shelf_drop {
                self.chrome.shelf_drop = over_shelf;
                self.pending_redraw = true;
            }
            let new_prev = if over_shelf { None } else { self.compute_drop_preview() };
            let changed = match (self.drop_preview.as_ref(), new_prev.as_ref()) {
                (Some(a), Some(b)) =>
                    window_id(&a.target) != window_id(&b.target) || a.t_rect != b.t_rect
                        || a.ghost != b.ghost || a.root_edge != b.root_edge || a.join != b.join,
                (None, None) => false,
                _ => true,
            };
            if changed { self.drop_preview = new_prev; self.relayout(); }
        }
    }

    /// Click-to-focus: focus the window under the cursor (layer surfaces
    /// like the bar receive pointer events but never steal keyboard focus).
    pub fn focus_window_at_cursor(&mut self) {
        let pos = self.pointer_location;
        let Some(window) = self.window_under(pos).map(|(w, _)| w) else { return };
        let ws = self.workspaces.active();
        if ws.floating.iter().any(|(w, _)| w == &window) {
            ws.focus_floating = Some(window.clone());
        } else if ws.tree.contains(&window) {
            ws.focus_floating = None;
            ws.tree.focus_window(&window);
        }
        self.update_keyboard_focus();
    }

    /// Begin a Super+drag grab on the window under the pointer, mirroring the
    /// mouse rules: left (0x110) moves, right (0x111) resizes; only floating
    /// windows move/resize freely, tiled windows take a right-drag split-ratio
    /// trade. Returns true if a grab started. Shared by the mouse PointerButton
    /// handler and the touch emulation (touch always passes left = move).
    pub fn try_begin_super_drag(&mut self, code: u32) -> bool {
        const BTN_LEFT:  u32 = 0x110;
        const BTN_RIGHT: u32 = 0x111;
        let pos = self.pointer_location;
        let Some(window) = self.window_under(pos).map(|(w, _)| w) else { return false };
        let floating_rect = self.workspaces.active_ref().floating.iter()
            .find(|(w, _)| w == &window).map(|(_, r)| *r);
        let tiled = floating_rect.is_none() && self.workspaces.active_ref().tree.contains(&window);

        // Tiled + left-drag: detach the window in place so it follows the cursor,
        // then re-tile it at the drop target on release (drag-to-rearrange).
        // A grouped tile leaves its group first; its siblings keep the slot.
        if tiled && code == BTN_LEFT && self.pull_grouped(&window) {
            return true;
        }
        if tiled && code == BTN_LEFT {
            let geo = self.space.element_geometry(&window).unwrap_or_else(|| {
                Rectangle::new((pos.x as i32 - 300, pos.y as i32 - 20).into(), (600, 400).into())
            });
            self.float_window_at(&window, geo);
            self.drag = Some(Drag {
                window, resize: false, tile_resize: false, from_tile: true,
                start_ptr: pos, start_rect: geo,
                started: std::time::Instant::now(), last_apply: std::time::Instant::now(),
                vel: (0.0, 0.0), last_motion: std::time::Instant::now(), last_ptr: pos,
            });
            return true;
        }

        let drag = match (floating_rect, tiled, code) {
            (Some(start_rect), _, _) => Some(Drag {
                window: window.clone(), resize: code == BTN_RIGHT, tile_resize: false,
                start_ptr: pos, start_rect, started: std::time::Instant::now(),
                last_apply: std::time::Instant::now(), from_tile: false,
                vel: (0.0, 0.0), last_motion: std::time::Instant::now(), last_ptr: pos,
            }),
            (None, true, BTN_RIGHT) => Some(Drag {
                window: window.clone(), resize: true, tile_resize: true,
                start_ptr: pos, start_rect: Default::default(), started: std::time::Instant::now(),
                last_apply: std::time::Instant::now(), from_tile: false,
                vel: (0.0, 0.0), last_motion: std::time::Instant::now(), last_ptr: pos,
            }),
            _ => None,
        };
        if let Some(drag) = drag {
            self.focus_window_at_cursor();
            self.drag = Some(drag);
            true
        } else {
            false
        }
    }

    /// The live drop preview for the in-flight tile drag: the hovered tile split
    /// in half by cursor position. None when not over a droppable tile. Uses
    /// `tile_geos` (the live layout, which already excludes the floating window).
    fn compute_drop_preview(&self) -> Option<DropPreview> {
        let drag = self.drag.as_ref()?;
        if drag.resize { return None; }
        // Tiled rearrange gets the full preview (edge zones + per-tile split); a
        // floating window only aero-snaps at the workspace EDGES (edge_only).
        self.drop_preview_at(self.pointer_location, &drag.window, !drag.from_tile)
    }

    /// The drop preview for an arbitrary point and dragged window. Split out so
    /// throw-to-snap can probe a *projected* release point (cursor + flick
    /// momentum) without depending on `self.drag` still being live. `edge_only`
    /// restricts to workspace-edge snaps (used for floating windows).
    fn drop_preview_at(&self, pos: Point<f64, Logical>, dragged: &Window, edge_only: bool) -> Option<DropPreview> {
        let did = window_id(dragged);
        use crate::layout::Direction;
        let gap = self.config.theme.gap;

        // ── workspace EDGE zones (checked first) ────────────────────────────
        // Cursor within EDGE px of a SCREEN edge → the window takes that whole
        // column/row (a root-level split). Hit-test against the full output (not
        // the inset tiling viewport) so pushing to the very edge — past the gap —
        // still triggers; the ghost/reflow stay viewport-based. Only when there's
        // an existing layout to push aside.
        if let Some(vp) = self.tiling_viewport() {
            let others = self.tile_geos.keys().filter(|id| **id != did).count();
            let screen = self.focused_output()
                .and_then(|o| self.space.output_geometry(&o));
            tracing::info!(edge_only, others, tile_geos = self.tile_geos.len(),
                screen_some = screen.is_some(), "drop_preview_at edge entry");
            if let Some(g) = screen {
                let (gx, gy) = (g.loc.x as f64, g.loc.y as f64);
                let (gw, gh) = (g.size.w as f64, g.size.h as f64);
                const EDGE: f64 = 64.0;   // hot-zone depth from the screen edge
                let inset = |r: Rectangle<i32, Logical>| Rectangle::new(
                    (r.loc.x + gap / 2, r.loc.y + gap / 2).into(),
                    ((r.size.w - gap).max(1), (r.size.h - gap).max(1)).into());
                // With existing tiles the snapped window takes HALF (a column/row);
                // with none it just fills the area (becomes the sole tile).
                let hw = if others >= 1 { vp.size.w / 2 } else { vp.size.w };
                let hh = if others >= 1 { vp.size.h / 2 } else { vp.size.h };
                let edge = if pos.x < gx + EDGE {
                    Some((Direction::Horizontal, true,  Rectangle::new(vp.loc, (hw, vp.size.h).into())))
                } else if pos.x > gx + gw - EDGE {
                    Some((Direction::Horizontal, false, Rectangle::new((vp.loc.x + vp.size.w - hw, vp.loc.y).into(), (hw, vp.size.h).into())))
                } else if pos.y < gy + EDGE {
                    Some((Direction::Vertical, true,  Rectangle::new(vp.loc, (vp.size.w, hh).into())))
                } else if pos.y > gy + gh - EDGE {
                    Some((Direction::Vertical, false, Rectangle::new((vp.loc.x, vp.loc.y + vp.size.h - hh).into(), (vp.size.w, hh).into())))
                } else { None };
                tracing::info!(?pos, gx, gy, gw, gh, others, edge_only,
                    matched = edge.is_some(), "edge_zone_check");
                if let Some((dir, before, raw)) = edge {
                    let ghost = inset(raw);
                    return Some(DropPreview {
                        target: dragged.clone(), t_rect: ghost, ghost, dir, before, root_edge: true, join: false });
                }
            }
        }

        // Floating aero-snap only engages at the edges — never split a tile.
        if edge_only { return None; }

        // ── tab groups: onto a strip, or the middle of a tile → join ────────
        if let Some(s) = self.chrome.strips.iter()
            .find(|s| s.rect.to_f64().contains(pos) && &s.active != dragged)
        {
            let tile = self.tile_geos.get(&window_id(&s.active)).copied().unwrap_or(s.rect);
            let ghost = Rectangle::new(s.rect.loc,
                (s.rect.size.w, tile.loc.y + tile.size.h - s.rect.loc.y).into());
            return Some(DropPreview { target: s.active.clone(), t_rect: tile, ghost,
                dir: Direction::Horizontal, before: false, root_edge: false, join: true });
        }
        if let Some((tid, r)) = self.tile_geos.iter()
            .filter(|(id, _)| **id != did)
            .find(|(_, rr)| {
                // the middle third (both axes) of a tile
                let (mx, my) = (rr.size.w as f64 / 3.0, rr.size.h as f64 / 3.0);
                pos.x > rr.loc.x as f64 + mx && pos.x < (rr.loc.x + rr.size.w) as f64 - mx
                    && pos.y > rr.loc.y as f64 + my && pos.y < (rr.loc.y + rr.size.h) as f64 - my
            })
            .map(|(id, r)| (*id, *r))
        {
            let target = self.workspaces.active_ref().tree.windows()
                .into_iter().find(|w| window_id(w) == tid)?;
            let ghost = self.chrome.strips.iter().find(|s| s.active == target)
                .map(|s| Rectangle::new(s.rect.loc, (r.size.w, r.loc.y + r.size.h - s.rect.loc.y).into()))
                .unwrap_or(r);
            return Some(DropPreview { target, t_rect: r, ghost,
                dir: Direction::Horizontal, before: false, root_edge: false, join: true });
        }

        // ── otherwise: split the hovered tile in half ───────────────────────
        let (tid, r) = self.tile_geos.iter()
            .filter(|(id, _)| **id != did)
            .find(|(_, rr)| rr.to_f64().contains(pos))
            .map(|(id, r)| (*id, *r))?;
        let target = self.workspaces.active_ref().tree.windows()
            .into_iter().find(|w| window_id(w) == tid)?;
        // Split the tile along its longer axis; the cursor's half is the ghost.
        let (dir, before, t_rect, ghost) = if r.size.w >= r.size.h {
            let half = ((r.size.w - gap) / 2).max(1);
            let left  = Rectangle::new(r.loc, (half, r.size.h).into());
            let right = Rectangle::new(
                (r.loc.x + r.size.w - half, r.loc.y).into(), (half, r.size.h).into());
            if pos.x < (r.loc.x + r.size.w / 2) as f64 {
                (Direction::Horizontal, true, right, left)
            } else {
                (Direction::Horizontal, false, left, right)
            }
        } else {
            let half = ((r.size.h - gap) / 2).max(1);
            let top    = Rectangle::new(r.loc, (r.size.w, half).into());
            let bottom = Rectangle::new(
                (r.loc.x, r.loc.y + r.size.h - half).into(), (r.size.w, half).into());
            if pos.y < (r.loc.y + r.size.h / 2) as f64 {
                (Direction::Vertical, true, bottom, top)
            } else {
                (Direction::Vertical, false, top, bottom)
            }
        };
        Some(DropPreview { target, t_rect, ghost, dir, before, root_edge: false, join: false })
    }

    /// Finish a drag: re-tile a detached (from_tile) window where the live
    /// preview showed it — splitting the hovered tile on the previewed side.
    /// An empty drop re-tiles at the focused slot. Non-tiling drags just ease.
    pub fn drop_dragged(&mut self, drag: Drag) {
        let mut preview = self.drop_preview.take();
        let over_shelf = std::mem::take(&mut self.chrome.shelf_drop);
        if drag.resize { return; }   // tiled/floating resizes settle themselves

        // Dropped on the stage shelf → stash it there, at the drop position.
        if over_shelf && !self.chrome.pinned.contains(&drag.window) {
            let at = self.shelf_drop_index();
            self.stash(&drag.window, at);
            return;
        }

        // Throw-to-snap: released over empty space, but flicked hard — project the
        // release point along the flick and land there, so a quick toss snaps home
        // without pixel-precise aim. Floating drags only probe EDGES (edge_only).
        if preview.is_none() {
            let speed = (drag.vel.0 * drag.vel.0 + drag.vel.1 * drag.vel.1).sqrt();
            const FLICK: f64 = 900.0;      // px/s — above this is a deliberate toss
            const LOOKAHEAD: f64 = 0.14;   // project ~140ms of travel ahead
            if speed > FLICK {
                let projected = Point::from((
                    self.pointer_location.x + drag.vel.0 * LOOKAHEAD,
                    self.pointer_location.y + drag.vel.1 * LOOKAHEAD,
                ));
                preview = self.drop_preview_at(projected, &drag.window, !drag.from_tile);
            }
        }

        tracing::info!(from_tile = drag.from_tile, resize = drag.resize,
            preview_some = preview.is_some(),
            root_edge = preview.as_ref().map(|p| p.root_edge),
            "drop_dragged");
        // Floating window: aero-snap into the tiling ONLY if it landed on an edge
        // zone; otherwise leave it floating where it was dropped.
        if !drag.from_tile {
            match preview.filter(|p| p.root_edge) {
                Some(p) => {
                    let ws = self.workspaces.active();
                    ws.floating.retain(|(w, _)| w != &drag.window);
                    ws.focus_floating = None;
                    ws.tree.insert_at_root_edge(drag.window.clone(), p.dir, p.before);
                    self.drag_release = Some((drag.window, std::time::Instant::now()));
                    self.relayout();
                    self.update_keyboard_focus();
                    self.emit_workspaces();
                }
                None => self.drag_release = Some((drag.window, std::time::Instant::now())),
            }
            return;
        }

        // Onto a tab strip / a tile's middle → it joins that tab group.
        if let Some(p) = preview.as_ref().filter(|p| p.join) {
            let target = p.target.clone();
            self.join_group(&target, &drag.window);
            self.drag_release = Some((drag.window, std::time::Instant::now()));
            self.relayout();
            self.update_keyboard_focus();
            self.emit_workspaces();
            return;
        }

        // Tiled (detached) window: re-tile where the live preview showed it.
        let ws = self.workspaces.active();
        ws.floating.retain(|(w, _)| w != &drag.window);
        ws.focus_floating = None;
        if let Some(p) = preview {
            if p.root_edge {
                // Full-span edge column/row → wrap the whole tree at the root.
                ws.tree.insert_at_root_edge(drag.window.clone(), p.dir, p.before);
            } else {
                ws.tree.focus_window(&p.target);
                ws.tree.next_split_override = Some(p.dir);
                ws.tree.insert(drag.window.clone());  // splits target → [target, dragged]
                if p.before {
                    // ghost was on the left/top → put the dragged window first
                    ws.tree.swap_windows(&p.target, &drag.window);
                }
            }
        } else {
            ws.tree.insert(drag.window.clone());  // no target — re-tile at focus
        }
        self.drag_release = Some((drag.window, std::time::Instant::now()));
        self.relayout();
        self.update_keyboard_focus();
        self.emit_workspaces();
    }

    /// Direct-manipulation tiling: grab the visible seam between two tiled
    /// windows with the BARE mouse (no Super) and drag to retrade their split
    /// ratios — like dragging a tile border in Hyprland/KDE. Detects a draggable
    /// seam when the pointer sits within ~gap px of a tile edge that has a
    /// neighbor on the far side; resizes the near (left/top) window's split.
    /// Returns true if a seam grab started (the press is then swallowed).
    pub fn try_begin_seam_resize(&mut self) -> bool {
        let pos = self.pointer_location;
        if let Some((w, _)) = self.seam_at_pointer() {
            self.begin_tile_resize(w, pos);
            return true;
        }
        false
    }

    /// Hit-test the pointer against the gaps between tiled windows. Returns the
    /// near (left/top) window plus the resize orientation (`true` = vertical
    /// seam → horizontal EW resize; `false` = horizontal seam → NS resize) when
    /// the pointer sits in a draggable seam. Backs both the bare-mouse seam grab
    /// and the hover resize-cursor affordance.
    pub fn seam_at_pointer(&self) -> Option<(Window, bool)> {
        let pos = self.pointer_location;
        let (px, py) = (pos.x, pos.y);
        // Stay in the gap between tiles (+a small tolerance) so we never steal a
        // click that's meant for window content near the edge.
        let gap = (self.config.theme.gap as f64).max(4.0);
        let tol = 4.0;
        // A grouped tile's tab strip is part of the tile: seams are measured
        // from the top of its strip, not the window under it.
        let tiled: Vec<(Window, Rectangle<i32, Logical>)> =
            self.workspaces.active_ref().tree.windows().into_iter()
                .filter_map(|w| self.space.element_geometry(&w).map(|mut g| {
                    if let Some(s) = self.chrome.strips.iter().find(|s| s.active == w) {
                        g.size.h += g.loc.y - s.rect.loc.y;
                        g.loc.y = s.rect.loc.y;
                    }
                    (w, g)
                }))
                .collect();
        if tiled.len() < 2 { return None; }

        for (w, g) in &tiled {
            let right  = (g.loc.x + g.size.w) as f64;
            let bottom = (g.loc.y + g.size.h) as f64;
            let span_y = py >= g.loc.y as f64 && py <= bottom;
            let span_x = px >= g.loc.x as f64 && px <= right;
            // vertical seam → horizontal resize of this (left) window. The gap
            // to the right runs [right, right+gap]; grab inside it (+tol).
            if span_y && px >= right - tol && px <= right + gap + tol {
                let has_right = tiled.iter().any(|(_, o)| {
                    let ol = o.loc.x as f64;
                    ol >= right && ol <= right + gap + tol
                        && py >= o.loc.y as f64 && py <= (o.loc.y + o.size.h) as f64
                });
                if has_right { return Some((w.clone(), true)); }
            }
            // horizontal seam → vertical resize of this (top) window
            if span_x && py >= bottom - tol && py <= bottom + gap + tol {
                let has_below = tiled.iter().any(|(_, o)| {
                    let ot = o.loc.y as f64;
                    ot >= bottom && ot <= bottom + gap + tol
                        && px >= o.loc.x as f64 && px <= (o.loc.x + o.size.w) as f64
                });
                if has_below { return Some((w.clone(), false)); }
            }
        }
        None
    }

    /// After a plain pointer move, show a resize cursor when hovering a seam so
    /// the drag-to-resize affordance is discoverable. Only overrides the cursor
    /// when no client owns it (the gap is compositor background); reverts to the
    /// arrow when leaving a seam over that same background.
    pub fn update_seam_cursor(&mut self) {
        use smithay::input::pointer::{CursorIcon, CursorImageStatus};
        // A client surface under the pointer owns the cursor — don't fight it.
        if matches!(self.cursor_status, CursorImageStatus::Surface(_)) { return; }
        match self.seam_at_pointer() {
            Some((_, horizontal)) => {
                let icon = if horizontal { CursorIcon::EwResize } else { CursorIcon::NsResize };
                self.cursor_status = CursorImageStatus::Named(icon);
            }
            None => {
                // Revert a lingering resize cursor once we leave the seam.
                if matches!(self.cursor_status,
                    CursorImageStatus::Named(CursorIcon::EwResize | CursorIcon::NsResize))
                {
                    self.cursor_status = CursorImageStatus::default_named();
                }
            }
        }
    }

    fn begin_tile_resize(&mut self, window: Window, pos: Point<f64, Logical>) {
        self.workspaces.active().tree.focus_window(&window);
        self.update_keyboard_focus();
        self.drag = Some(Drag {
            window, resize: true, tile_resize: true,
            start_ptr: pos, start_rect: Default::default(),
            started: std::time::Instant::now(),
            last_apply: std::time::Instant::now(), from_tile: false,
            vel: (0.0, 0.0), last_motion: std::time::Instant::now(), last_ptr: pos,
        });
    }

    // ── touchscreen → pointer emulation ──────────────────────────────────────
    // vendiOS is a laptop-with-touchscreen target, so touch acts like the mouse.
    // ~8px slop separates a tap from a drag; a 450ms still hold is a right click.
    fn emul_motion(&mut self, pos: Point<f64, Logical>, time: u32) {
        let Some(pointer) = self.seat.get_pointer() else { return };
        self.pointer_location = pos;
        // A finger on a tab / shelf card: chrome owns it (and may turn it into
        // a tear-off drag, which then follows the finger like a mouse drag).
        if self.drag.is_some() && self.chrome.press.is_none() && self.touch_chrome_drag {
            self.drag_update();
            return;
        }
        if self.chrome_motion() {
            if self.drag.is_some() { self.touch_chrome_drag = true; }
            self.pending_redraw = true;
            return;
        }
        let under = self.surface_under(pos);
        pointer.motion(self, under, &MotionEvent {
            location: pos, serial: SERIAL_COUNTER.next_serial(), time,
        });
        pointer.frame(self);
        self.pending_redraw = true;
    }

    fn emul_button(&mut self, code: u32, pressed: bool, time: u32) {
        let Some(pointer) = self.seat.get_pointer() else { return };
        if !pressed && std::mem::take(&mut self.touch_chrome_drag) {
            if let Some(drag) = self.drag.take() { self.drop_dragged(drag); }
            self.pending_redraw = true;
            return;
        }
        if self.chrome_button(code, pressed) {
            self.pending_redraw = true;
            return;
        }
        pointer.button(self, &ButtonEvent {
            button: code,
            state:  if pressed { ButtonState::Pressed } else { ButtonState::Released },
            serial: SERIAL_COUNTER.next_serial(),
            time,
        });
        pointer.frame(self);
    }

    /// Abandon an in-progress single-finger emulation (a second finger landed,
    /// or the gesture was cancelled): release a held button / end a move grab.
    fn cancel_touch_emul(&mut self, time: u32) {
        let Some(t) = self.touch.take() else { return };
        match t.phase {
            TouchPhase::Dragging => self.emul_button(0x110, false, time),
            TouchPhase::WindowMove => {
                if let Some(drag) = self.drag.take() { self.drop_dragged(drag); }
                self.pending_redraw = true;
            }
            _ => {}
        }
    }

    /// Forget all touch state (TouchCancel, or session lock).
    pub fn touch_reset(&mut self) {
        self.touch = None;
        self.touch_points.clear();
        self.touch_gesture = None;
    }

    /// First finger down. `super_held` mirrors the keyboard Super modifier — if
    /// set and a window is grabbed, this becomes a window move (no client press).
    pub fn touch_down(
        &mut self,
        slot: smithay::backend::input::TouchSlot,
        pos: Point<f64, Logical>,
        time: u32,
        super_held: bool,
    ) {
        self.touch_points.insert(slot, pos);
        tracing::debug!(points = self.touch_points.len(), ?pos, "touch down");
        // Two or more fingers → multi-finger gesture, not a pointer. Drop any
        // single-finger emulation already underway and track the gesture.
        if self.touch_points.len() >= 2 || self.touch_gesture.is_some() {
            self.cancel_touch_emul(time);
            let fingers = self.touch_gesture.map(|g| g.fingers.max(self.touch_points.len()))
                .unwrap_or(self.touch_points.len());
            self.touch_gesture = Some(TouchGesture { fingers, dx: 0.0, dy: 0.0, fired: false });
            tracing::debug!(fingers, "touch gesture begin");
            return;
        }
        if self.touch.is_some() { return; }
        self.emul_motion(pos, time);
        self.focus_window_at_cursor();
        let phase = if super_held && self.try_begin_super_drag(0x110) {
            TouchPhase::WindowMove
        } else {
            TouchPhase::Pending
        };
        self.touch = Some(TouchEmul {
            slot, down_pos: pos, down_time: time, down_instant: std::time::Instant::now(),
            cur_pos: pos, phase, from_edge: pos.y <= 40.0,
        });
    }

    pub fn touch_motion(
        &mut self,
        slot: smithay::backend::input::TouchSlot,
        pos: Point<f64, Logical>,
        time: u32,
    ) {
        const SLOP: f64 = 8.0;
        // Multi-finger gesture: accumulate each finger's travel, no pointer.
        let prev = self.touch_points.insert(slot, pos);
        if let Some(g) = self.touch_gesture.as_mut() {
            if let Some(prev) = prev { g.dx += pos.x - prev.x; g.dy += pos.y - prev.y; }
            return;
        }
        let Some(t) = self.touch.as_ref() else { return };
        if t.slot != slot { return; }
        let phase = t.phase;
        let down = t.down_pos;
        let from_edge = t.from_edge;
        if let Some(t) = self.touch.as_mut() { t.cur_pos = pos; }
        // Top-edge pull (swipe down from the top): from the right third opens
        // the control center (it lives in the right notch), otherwise the
        // dashboard (it expands from the center notch).
        if from_edge && phase == TouchPhase::Pending && pos.y - down.y > 50.0 {
            if let Some(t) = self.touch.as_mut() { t.phase = TouchPhase::Consumed; }
            let geo = self.space.outputs().next().and_then(|o| self.space.output_geometry(o));
            let frac = geo.map(|g| (down.x - g.loc.x as f64) / g.size.w.max(1) as f64).unwrap_or(0.5);
            let cmd = if frac > 0.66 {
                "quickshell -c vendibar-pro ipc call panel showControl"
            } else {
                "quickshell -c vendibar-pro ipc call dash toggle"
            };
            tracing::debug!(frac, %cmd, "touch edge-pull");
            let _ = crate::spawn_reaped(std::process::Command::new("sh").arg("-c").arg(cmd));
            return;
        }
        match phase {
            TouchPhase::WindowMove => { self.pointer_location = pos; self.drag_update(); }
            TouchPhase::Pending => {
                let dx = pos.x - down.x;
                let dy = pos.y - down.y;
                // A downward, vertically-dominant pull from the top edge is a
                // control-center gesture (handled above at 50px) — don't let it
                // turn into a left-drag before it gets there.
                let edge_pull = from_edge && dy > 0.0 && dy > dx.abs();
                if dx.hypot(dy) > SLOP && !edge_pull {
                    // Crossed into a drag: press left at the start, then track.
                    if let Some(t) = self.touch.as_mut() { t.phase = TouchPhase::Dragging; }
                    self.emul_button(0x110, true, time);
                }
                self.emul_motion(pos, time);
            }
            TouchPhase::Dragging => self.emul_motion(pos, time),
            TouchPhase::Consumed => {}
        }
    }

    pub fn touch_up(
        &mut self,
        slot: smithay::backend::input::TouchSlot,
        time: u32,
    ) {
        self.touch_points.remove(&slot);
        // Resolve a multi-finger gesture as fingers lift: 3-finger horizontal
        // swipe switches workspace. Summed travel ≈ fingers × per-finger dist.
        if let Some(g) = self.touch_gesture {
            if !g.fired && g.fingers >= 3 {
                let thr = 60.0 * g.fingers as f64;
                let horiz = g.dx.abs() >= thr && g.dx.abs() > g.dy.abs();
                tracing::debug!(fingers = g.fingers, dx = g.dx, dy = g.dy, thr, horiz, "touch gesture resolve");
                if horiz {
                    let forward = g.dx < 0.0; // swipe left → next workspace
                    let adj = self.workspaces.adjacent_id(forward);
                    tracing::debug!(forward, ?adj, "3-finger → workspace");
                    if let Some(id) = adj {
                        self.switch_workspace(id);
                    }
                    if let Some(g) = self.touch_gesture.as_mut() { g.fired = true; }
                }
            }
            if self.touch_points.is_empty() { self.touch_gesture = None; }
            return;
        }
        let Some(t) = self.touch.as_ref() else { return };
        if t.slot != slot { return; }
        let phase = t.phase;
        let pos = t.cur_pos;
        self.touch = None;
        match phase {
            TouchPhase::Pending => {
                // A tap: a full left click in place.
                self.emul_motion(pos, time);
                self.emul_button(0x110, true, time);
                self.emul_button(0x110, false, time);
            }
            TouchPhase::Dragging => self.emul_button(0x110, false, time),
            TouchPhase::WindowMove => {
                if let Some(drag) = self.drag.take() { self.drop_dragged(drag); }
                self.pending_redraw = true;
            }
            TouchPhase::Consumed => {}
        }
    }

    /// Per-tick check: a finger held still past the long-press threshold fires a
    /// right click (context menu), then swallows input until the finger lifts.
    pub fn touch_tick(&mut self) {
        const LONG_PRESS_MS: u128 = 450;
        let Some(t) = self.touch.as_ref() else { return };
        if t.phase != TouchPhase::Pending { return; }
        if t.from_edge { return; } // edge touches are control-center pulls, not right-clicks
        if t.down_instant.elapsed().as_millis() < LONG_PRESS_MS { return; }
        let pos = t.cur_pos;
        let time = t.down_time.wrapping_add(LONG_PRESS_MS as u32);
        if let Some(t) = self.touch.as_mut() { t.phase = TouchPhase::Consumed; }
        self.emul_motion(pos, time);
        self.emul_button(0x111, true, time);
        self.emul_button(0x111, false, time);
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
    /// Re-read the config file and apply it live (keyboard, outputs, theme,
    /// binds). Shared by the `reload-config` keybind and the `vendi-ctl reload`
    /// IPC. Mode/refresh changes are flagged for the udev backend next tick.
    pub fn reload_config(&mut self) -> anyhow::Result<()> {
        let cfg = crate::config::Config::load()?;
        self.config = cfg;
        self.wallpaper_gen += 1;
        self.pending_redraw = true;
        if let Some(kb) = self.seat.get_keyboard() {
            let (layout, variant, options, delay, rate) = (
                self.config.kb_layout.clone(),
                self.config.kb_variant.clone(),
                self.config.kb_options.clone(),
                self.config.repeat_delay, self.config.repeat_rate,
            );
            if let Err(e) = kb.set_xkb_config(self, smithay::input::keyboard::XkbConfig {
                layout: &layout, variant: &variant,
                options: if options.is_empty() { None } else { Some(options) },
                ..Default::default()
            }) {
                tracing::warn!(?e, "set xkb config on reload");
            }
            kb.change_repeat_info(rate, delay);
        }
        self.write_kb_layout();
        let cfgs = self.config.outputs.clone();
        let outs: Vec<_> = self.space.outputs().cloned().collect();
        for o in outs {
            match cfgs.iter().find(|c| c.name == o.name()) {
                Some(c) => {
                    let scale = c.scale.map(|s| if s.fract().abs() < 1e-6 {
                        smithay::output::Scale::Integer(s.max(1.0) as i32)
                    } else {
                        smithay::output::Scale::Fractional(s)
                    });
                    o.change_current_state(None, None, scale, c.position.map(|p| p.into()));
                    if let Some(p) = c.position { self.space.map_output(&o, p); }
                }
                None => o.change_current_state(
                    None, None, Some(smithay::output::Scale::Integer(1)), None),
            }
            smithay::desktop::layer_map_for_output(&o).arrange();
        }
        self.pending_output_modes = true;
        self.relayout();
        // NOTE: pointer/touchpad device config (natural scroll, accel…) is
        // applied at device-add time; a live reload won't re-apply it to
        // already-connected devices (takes effect next session). Binds, theme,
        // keyboard, and outputs all apply immediately.
        tracing::info!("config reloaded");
        Ok(())
    }

    pub fn run_action(&mut self, action: crate::input::Action) -> bool {
        use crate::input::Action::*;
        match action {
            Spawn(cmd) => {
                tracing::info!(%cmd, "spawn");
                // Detach so vendiwm doesn't accumulate zombies.
                if let Err(e) = crate::spawn_reaped(std::process::Command::new("sh").arg("-c").arg(&cmd)) {
                    tracing::warn!(?e, %cmd, "spawn failed");
                }
            }
            Close => {
                if let Some(w) = self.focused_window() {
                    if let Some(t) = w.toplevel() {
                        t.send_close();
                    }
                    // X11 windows have no xdg toplevel — send WM_DELETE_WINDOW.
                    #[cfg(feature = "xwayland")]
                    if let Some(x) = w.x11_surface() {
                        let _ = x.close();
                    }
                }
            }
            Kill => {
                if let Some(w) = self.focused_window() {
                    // Resolve the owning process and SIGKILL it so a frozen/hung
                    // window dies even though it ignores the polite Close above.
                    // CRITICAL: for X11 windows the wl_surface's client is
                    // XWayland itself (shared by every X app) — killing that PID
                    // takes down the whole Xserver and panics the wm. So use the
                    // X11 window's own _NET_WM_PID instead; fall back to a polite
                    // X11 close if it isn't advertised.
                    #[cfg(feature = "xwayland")]
                    let x11 = w.x11_surface().cloned();
                    #[cfg(not(feature = "xwayland"))]
                    let x11: Option<()> = None;
                    let pid = match &x11 {
                        #[cfg(feature = "xwayland")]
                        Some(x) => { let _ = x.close(); x.pid() }
                        _ => w.wl_surface()
                            .and_then(|s| s.client())
                            .and_then(|c| c.get_credentials(&self.display_handle).ok())
                            .map(|creds| creds.pid as u32),
                    };
                    if let Some(pid) = pid {
                        let _ = crate::spawn_reaped(std::process::Command::new("kill")
                            .arg("-9").arg(pid.to_string()));
                    }
                    // Drop the tile immediately so the desktop unwedges even if
                    // the client's destroy callback is slow or never arrives.
                    self.workspaces.remove_window(&w);
                    self.space.unmap_elem(&w);
                    let id = window_id(&w);
                    self.window_titles.remove(&id);
                    self.last_geos.remove(&id);
                    self.rule_checked.remove(&id);
                    self.pending_ipc_events.push(crate::ipc::Event::WindowClosed { id });
                    self.relayout();
                    self.update_keyboard_focus();
                    self.emit_workspaces();
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
            MoveToWorkspaceFollow(n) => {
                self.move_focused_to_workspace(n);
                self.switch_workspace(n);
            }
            WorkspaceNext       => {
                if let Some(id) = self.workspaces.adjacent_id(true) { self.switch_workspace(id); }
            }
            WorkspacePrev       => {
                if let Some(id) = self.workspaces.adjacent_id(false) { self.switch_workspace(id); }
            }
            WorkspaceLast       => {
                let prev = self.workspaces.previous_id();
                self.switch_workspace(prev);
            }
            ReloadConfig        => {
                if let Err(e) = self.reload_config() { tracing::warn!(?e, "reload-config failed"); }
            }
            CenterFloating      => self.center_floating(),
            CycleLayout         => {
                let m = self.workspaces.active().mode.next();
                self.workspaces.active().mode = m;
                self.relayout();
                self.update_keyboard_focus();
                // brief toast so you know which layout you're in
                let _ = crate::spawn_reaped(std::process::Command::new("notify-send")
                    .args(["-a", "Layout", "-t", "1200", "Layout", m.label()]));
            }
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
            CycleKbLayout       => {
                if let Some(kb) = self.seat.get_keyboard() {
                    kb.with_xkb_state(self, |mut ctx| ctx.cycle_next_layout());
                }
                // The bar watches $XDG_RUNTIME_DIR/vendiwm-kblayout and shows a
                // notch popup on change — no notify-send needed.
                self.write_kb_layout();
            }
            MoveToScratchpad    => self.move_to_scratchpad(),
            ToggleScratchpad    => self.toggle_scratchpad(),
            GroupOrCycle        => self.group_or_cycle(),
            StagePull           => self.stage_pull_next(),
            Pin                 => self.toggle_pin(),
            FocusOutput(d)      => self.focus_output_dir(d),
            MoveToOutput(d)     => self.move_to_output_dir(d),
            Ungroup             => self.ungroup(),
            Lock                => self.lock_session(),
            Quit => { self.quit_requested = true; return true; }
        }
        false
    }

    /// Publish the active keyboard layout for the bar to read. Writes a short
    /// upper-case code (e.g. "US"/"ES") to $XDG_RUNTIME_DIR/vendiwm-kblayout, or
    /// an empty file when only one layout is configured (so the bar hides the
    /// indicator). The bar watches this file (FileView) and shows it live.
    pub fn write_kb_layout(&mut self) {
        let Some(kb) = self.seat.get_keyboard() else { return };
        let (idx, count) = kb.with_xkb_state(self, |ctx| {
            let xkb = ctx.xkb().lock().unwrap();
            (xkb.active_layout().0, xkb.layouts().count())
        });
        let code = if count <= 1 {
            String::new()
        } else {
            self.config.kb_layout.split(',')
                .nth(idx as usize)
                .map(|s| s.trim().to_uppercase())
                .unwrap_or_default()
        };
        if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
            let path = std::path::Path::new(&dir).join("vendiwm-kblayout");
            let _ = std::fs::write(path, format!("{code}\n"));
        }
    }

    /// Match the workspace model to the connected monitors: a new monitor
    /// gets a desk, an unplugged one takes its desk off screen (a reconnect
    /// brings it back). Call after outputs are mapped/unmapped.
    pub fn sync_outputs(&mut self) {
        let names: Vec<String> = self.space.outputs().map(|o| o.name()).collect();
        let gone: Vec<String> = self.workspaces.shown().iter()
            .map(|(o, _)| o.clone())
            .filter(|o| !names.contains(o))
            .collect();
        for o in gone {
            for w in self.workspaces.output_removed(&o) { self.space.unmap_elem(&w); }
        }
        for n in &names { self.workspaces.output_added(n); }
        self.emit_workspaces();
    }

    /// The monitor absolute pointer/touch input maps onto: the laptop's
    /// built-in panel (that's where the touchscreen is), else the first one.
    pub fn touch_output(&self) -> Option<Output> {
        self.space.outputs()
            .find(|o| { let n = o.name(); n.starts_with("Embedded") || n.starts_with("eDP") || n.starts_with("LVDS") })
            .or_else(|| self.space.outputs().next())
            .cloned()
    }

    /// Which monitors show this surface (via its root: a mapped window or a
    /// layer surface). None when it can't be placed.
    fn surface_outputs(&self, surface: &WlSurface) -> Option<Vec<String>> {
        let mut root = surface.clone();
        while let Some(p) = smithay::wayland::compositor::get_parent(&root) { root = p; }
        if let Some(w) = self.space.elements().find(|w| w.wl_surface().is_some_and(|s| *s == root)) {
            return Some(self.space.outputs_for_element(w).iter().map(|o| o.name()).collect());
        }
        for o in self.space.outputs() {
            let map = layer_map_for_output(o);
            if map.layer_for_surface(&root, WindowSurfaceType::TOPLEVEL).is_some() {
                return Some(vec![o.name()]);
            }
        }
        None
    }

    /// The lock surface the locker made for this monitor.
    pub fn lock_surface_on(&self, output: &str) -> Option<&LockSurface> {
        self.lock_surfaces.iter().find(|(n, _)| n == output).map(|(_, s)| s)
    }

    /// Executable name of a window's client (/proc/<pid>/comm) — "" if unknown.
    pub fn window_process(&self, w: &Window) -> String {
        #[cfg(feature = "xwayland")]
        if let Some(x) = w.x11_surface() {
            return x.pid().and_then(|p| std::fs::read_to_string(format!("/proc/{p}/comm")).ok())
                .map(|s| s.trim().to_string()).unwrap_or_default();
        }
        w.wl_surface()
            .and_then(|s| s.client())
            .and_then(|c| c.get_credentials(&self.display_handle).ok())
            .and_then(|cr| std::fs::read_to_string(format!("/proc/{}/comm", cr.pid)).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    /// A monitor by connector name.
    pub fn output_named(&self, name: &str) -> Option<Output> {
        self.space.outputs().find(|o| o.name() == name).cloned()
    }

    /// The focused monitor — the one showing the active workspace (the
    /// pointer's monitor). Falls back to the first one.
    pub fn focused_output(&self) -> Option<Output> {
        self.output_named(self.workspaces.focused_output())
            .or_else(|| self.space.outputs().next().cloned())
    }

    /// The pointer crossed onto another monitor: that monitor's desk becomes
    /// the active one (keybinds, new windows and focus follow you there).
    pub fn update_output_focus(&mut self) {
        let pos = self.pointer_location;
        let Some(name) = self.space.output_under(pos).next().map(|o| o.name()) else { return };
        let old = self.workspaces.active_id();
        if name != self.workspaces.focused_output() && self.workspaces.focus_output(&name) {
            // A window being dragged comes along: it now belongs to the desk
            // on the monitor it was carried onto (drops land there).
            if let Some(w) = self.drag.as_ref().map(|d| d.window.clone()) {
                let entry = self.workspaces.get_mut_existing(old).and_then(|ws| {
                    let i = ws.floating.iter().position(|(x, _)| x == &w)?;
                    if ws.focus_floating.as_ref() == Some(&w) { ws.focus_floating = None; }
                    Some(ws.floating.remove(i))
                });
                if let Some(e) = entry {
                    let ws = self.workspaces.active();
                    ws.floating.push(e);
                    ws.focus_floating = Some(w);
                }
                self.drop_preview = None;
                self.emit_workspaces();
                self.pending_redraw = true;
                return;
            }
            // focus what's under the pointer on the new monitor, if anything
            if let Some(w) = self.window_under(pos).map(|(w, _)| w) {
                self.focus_window(&w);
            } else {
                self.update_keyboard_focus();
            }
            self.emit_workspaces();
            self.pending_redraw = true;
        }
    }

    /// Put the pointer in the middle of a monitor (keyboard jumps between
    /// monitors, `workspace N` onto a desk shown on the other screen).
    pub fn warp_to_output(&mut self, name: &str) {
        let Some(o) = self.output_named(name) else { return };
        let Some(g) = self.space.output_geometry(&o) else { return };
        self.pointer_location = (
            (g.loc.x + g.size.w / 2) as f64,
            (g.loc.y + g.size.h / 2) as f64,
        ).into();
        if let Some(pointer) = self.seat.get_pointer() {
            let under = self.surface_under(self.pointer_location).map(|(s, p)| (s.into(), p));
            let loc = self.pointer_location;
            pointer.motion(self, under, &MotionEvent {
                location: loc, serial: SERIAL_COUNTER.next_serial(), time: 0,
            });
            pointer.frame(self);
        }
        self.pending_redraw = true;
    }

    /// Is the stage shelf on this monitor?
    fn shelf_here(&self, output: &str) -> bool {
        self.shelf_visible() && self.shelf_output_name().as_deref() == Some(output)
    }

    /// Smart-gaps case: a single tiled window, nothing floating/fullscreen, and
    /// the feature enabled. The lone window fills the screen edge-to-edge (no
    /// outer margin, no inner gap) — like Hyprland's no_gaps_when_only_one.
    fn smart_single_on(&self, output: &str, ws_id: u32) -> bool {
        let Some(ws) = self.workspaces.get(ws_id) else { return false };
        self.config.theme.smart_gaps
            && !self.shelf_here(output)
            && ws.fullscreen.is_none()
            && ws.floating.is_empty()
            && ws.tree.windows().len() == 1
    }

    /// A monitor's area minus layer-shell exclusive zones (the bar), minus the
    /// outer gap. Tiles are later shrunk by GAP_IN/2 per side, so windows
    /// sit GAP_IN apart and GAP_OUT from the screen edge.
    fn viewport_on(&self, output: &Output, ws_id: u32) -> Option<Rectangle<i32, Logical>> {
        let geometry = self.space.output_geometry(output)?;
        let layer_map = layer_map_for_output(output);
        let non_exclusive = layer_map.non_exclusive_zone();
        drop(layer_map);
        let mut viewport = geometry;
        viewport.loc  += non_exclusive.loc;
        viewport.size  = non_exclusive.size;
        let name = output.name();
        let margin = if self.smart_single_on(&name, ws_id) { 0 }
                     else { self.config.theme.margin - self.config.theme.gap / 2 };
        viewport.loc.x  += margin;
        viewport.loc.y  += margin;
        viewport.size.w  = (viewport.size.w - margin * 2).max(1);
        viewport.size.h  = (viewport.size.h - margin * 2).max(1);
        // The stage shelf takes a column on the left; the stage keeps the rest.
        if self.shelf_here(&name) {
            viewport.loc.x += crate::chrome::SHELF_W;
            viewport.size.w = (viewport.size.w - crate::chrome::SHELF_W).max(1);
        }
        Some(viewport)
    }

    pub fn tiling_viewport_pub(&self) -> Option<Rectangle<i32, Logical>> { self.tiling_viewport() }

    /// The tiling area of the focused monitor's desk.
    fn tiling_viewport(&self) -> Option<Rectangle<i32, Logical>> {
        let output = self.focused_output()?;
        self.viewport_on(&output, self.workspaces.active_id())
    }

    /// Recompute every mapped window's rectangle: each monitor lays out the
    /// desk it's showing — tiled tree → that monitor's viewport, floating →
    /// stored rects (raised), fullscreen → that whole monitor, on top.
    pub fn relayout(&mut self) {
        self.workspaces.prune_dead();

        // Safety net: unmap anything that isn't on a desk that's on screen.
        let visible = self.workspaces.visible_windows();
        let stray: Vec<Window> = self.space.elements()
            .filter(|w| !visible.contains(*w))
            .cloned()
            .collect();
        for w in stray { self.space.unmap_elem(&w); }

        self.tile_geos.clear();
        self.chrome.strips.clear();
        let shown: Vec<(String, u32)> = self.workspaces.shown().to_vec();
        for (name, ws_id) in shown {
            if let Some(output) = self.output_named(&name) {
                self.layout_desk(&output, ws_id);
            }
        }

        // Pinned windows ride above the rest of the floating layer.
        self.chrome.pinned.retain(|w| w.alive());
        self.chrome.pinned_from_tile.retain(|w| w.alive());
        self.chrome.phones.retain(|(w, _)| w.alive());
        self.chrome.phone_drag.retain(|w| w.alive());
        self.raise_overlays();
        self.workspaces.scratchpad.retain(|w| w.alive());
        self.layout_shelf();

        self.pending_redraw = true;
    }

    /// Lay out one monitor's desk.
    fn layout_desk(&mut self, output: &Output, ws_id: u32) {
        let Some(geometry) = self.space.output_geometry(output) else { return };
        let Some(viewport) = self.viewport_on(output, ws_id) else { return };
        let Some(ws) = self.workspaces.get(ws_id) else { return };
        let focused_desk = ws_id == self.workspaces.active_id();
        const TILED: [xdg_toplevel::State; 4] = [
            xdg_toplevel::State::TiledLeft, xdg_toplevel::State::TiledRight,
            xdg_toplevel::State::TiledTop, xdg_toplevel::State::TiledBottom,
        ];
        let bounds = viewport.size;

        let gap = if self.smart_single_on(&output.name(), ws_id) { 0 } else { self.config.theme.gap };
        let mode = ws.mode;
        // Edge drop-zone: reflow the existing tree into the half NOT taken by the
        // full-span ghost column/row, so you see the exact post-drop layout.
        let mut viewport = viewport;
        if let Some(p) = self.drop_preview.as_ref().filter(|p| p.root_edge && focused_desk) {
            use crate::layout::Direction;
            let hw = viewport.size.w / 2;
            let hh = viewport.size.h / 2;
            match (p.dir, p.before) {
                (Direction::Horizontal, true)  => { viewport.loc.x += hw; viewport.size.w -= hw; }
                (Direction::Horizontal, false) => { viewport.size.w -= hw; }
                (Direction::Vertical, true)    => { viewport.loc.y += hh; viewport.size.h -= hh; }
                (Direction::Vertical, false)   => { viewport.size.h -= hh; }
            }
        }
        let layouts = ws.tree.placements(viewport, mode);
        // Grouped tiles give the top of their slot to the tab strip.
        let stacks: Vec<(Window, Vec<Window>)> = ws.stacks.iter()
            .filter_map(|s| s.active_window().map(|a| (a.clone(), s.members.clone())))
            .collect();
        let monocle_top = ws.tree.focused().cloned();
        let floating = ws.floating.clone();
        let fullscreen = ws.fullscreen.clone();
        for (window, mut rect) in layouts {
            // Inner gap: half per side so neighbors end up `gap` apart.
            rect.loc.x  += gap / 2;
            rect.loc.y  += gap / 2;
            rect.size.w  = (rect.size.w - gap).max(1);
            rect.size.h  = (rect.size.h - gap).max(1);
            if let Some((_, members)) = stacks.iter().find(|(a, _)| a == &window) {
                use crate::chrome::{TAB_H, TAB_GAP};
                if rect.size.h > (TAB_H + TAB_GAP) * 3 {
                    self.chrome.strips.push(crate::chrome::TabStrip {
                        rect: Rectangle::new(rect.loc, (rect.size.w, TAB_H).into()),
                        active: window.clone(),
                        members: members.clone(),
                    });
                    rect.loc.y += TAB_H + TAB_GAP;
                    rect.size.h -= TAB_H + TAB_GAP;
                }
            }
            self.tile_geos.insert(window_id(&window), rect);
            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|s| {
                    s.size = Some(rect.size);
                    // Tell the app it's tiled (the size is exact, not a hint)
                    // and how big its monitor's usable area is — browsers
                    // otherwise restore their last window size, e.g. a 2K
                    // size on a 1080p laptop screen.
                    s.bounds = Some(bounds);
                    for st in TILED { s.states.set(st); }
                });
                toplevel.send_configure();
            }
            // X11 windows are rootful: hand them the full rect (loc + size) so
            // the Xserver places and sizes them where the tile sits.
            #[cfg(feature = "xwayland")]
            if let Some(x11) = window.x11_surface() {
                let _ = x11.configure(Some(rect));
            }
            // Tile moved or resized → morph it over (only when already mapped).
            self.push_geo_anim(&window, rect);
            self.space.map_element(window, rect.loc, false);
        }

        // Monocle stacks every window at full size — raise the focused one so
        // it's the one you actually see (and keep it above its peers).
        if mode == crate::layout::LayoutMode::Monocle {
            if let Some(w) = monocle_top { self.space.raise_element(&w, true); }
        }

        // Live drop preview: shrink the hovered tile to its half so the ghost
        // placeholder has real space — you see the exact resulting layout.
        if let Some(p) = self.drop_preview.clone().filter(|_| focused_desk) {
            if !p.root_edge && !p.join && p.target.alive() {
                use crate::chrome::{TAB_H, TAB_GAP};
                if let Some(s) = self.chrome.strips.iter_mut().find(|s| s.active == p.target) {
                    s.rect = Rectangle::new((p.t_rect.loc.x, p.t_rect.loc.y - TAB_H - TAB_GAP).into(),
                        (p.t_rect.size.w, TAB_H).into());
                }
                if let Some(toplevel) = p.target.toplevel() {
                    toplevel.with_pending_state(|s| { s.size = Some(p.t_rect.size); });
                    toplevel.send_configure();
                }
                #[cfg(feature = "xwayland")]
                if let Some(x11) = p.target.x11_surface() { let _ = x11.configure(Some(p.t_rect)); }
                self.push_geo_anim(&p.target, p.t_rect);
                self.space.map_element(p.target.clone(), p.t_rect.loc, false);
            }
        }

        // Floating layer sits above tiled windows.
        for (window, rect) in floating {
            // A phone mirror keeps its own (video) size — that's how we learn
            // the stream's real shape, rotation included. It's drawn scaled
            // into `rect`. (Sizing it ourselves made the video sink letterbox
            // a portrait phone inside a landscape window.)
            let phone = self.is_phone(&window);
            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|s| {
                    s.size = if phone { None } else { Some(rect.size) };
                    s.bounds = Some(bounds);
                    for st in TILED { s.states.unset(st); }
                });
                toplevel.send_pending_configure();
            }
            #[cfg(feature = "xwayland")]
            if let Some(x11) = window.x11_surface() {
                let _ = x11.configure(Some(rect));
            }
            self.push_geo_anim(&window, rect);
            self.space.map_element(window.clone(), rect.loc, false);
            self.space.raise_element(&window, false);
        }

        // Fullscreen override covers the whole monitor (incl. the bar zone).
        if let Some(window) = fullscreen.filter(|w| w.alive()) {
            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|s| { s.size = Some(geometry.size); });
                toplevel.send_configure();
            }
            #[cfg(feature = "xwayland")]
            if let Some(x11) = window.x11_surface() {
                let _ = x11.configure(Some(geometry));
            }
            self.push_geo_anim(&window, geometry);
            self.space.map_element(window.clone(), geometry.loc, false);
            self.space.raise_element(&window, true);
        }
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
            self.drop_preview = None;
            self.drop_preview = None;
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
        // The overview opens on the focused monitor and shows that monitor's
        // desks; the other monitor keeps showing its desk untouched.
        let Some(output) = self.focused_output() else { return out };
        let Some(geo) = self.space.output_geometry(&output) else { return out };
        let active = self.workspaces.active_id();
        let here = output.name();

        let panels: Vec<u32> = self.workspaces.iter()
            .filter(|ws| ws.id == active || (!ws.is_empty() && ws.output == here
                && !self.workspaces.is_shown(ws.id)))
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
                let mine = ws.windows();
                for window in self.space.elements().filter(|w| mine.contains(*w)) {
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
        self.drop_preview = None;
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
        // The dragged window itself and a seam-resize must track the pointer
        // 1:1 (relayout fires every motion event; a spring would lag behind).
        // But OTHER tiles reflowing — the detach fill-in and the drop-preview
        // resize — should glide. (The morph also scales the buffer, so the
        // gliding tile never overdraws its moving edge.)
        if let Some(d) = &self.drag {
            if d.tile_resize || &d.window == window { return; }
        }
        let Some(old) = self.space.element_geometry(window) else { return };
        if old == target { return; }
        self.geo_anims.retain(|(w, _, _)| w != window);
        self.geo_anims.push((window.clone(), old, std::time::Instant::now()));
    }
}

/// Keyboard focus target. Must distinguish a Wayland surface from an X11 one:
/// smithay only sends WM_TAKE_FOCUS / sets X input focus from `X11Surface`'s
/// KeyboardTarget::enter, per the window's ICCCM input model. Focusing the bare
/// WlSurface (as we used to) skips that, so "Globally Active" X11 apps — most
/// games, e.g. Geometry Dash under Proton (input=False + WM_TAKE_FOCUS) — never
/// actually grab the keyboard. (Passive apps like Discord worked because
/// XWayland sets their X focus itself.)
#[derive(Debug, Clone, PartialEq)]
pub enum KbFocus {
    Wl(WlSurface),
    #[cfg(feature = "xwayland")]
    X11(smithay::xwayland::X11Surface),
}

impl KbFocus {
    pub fn wl_surface(&self) -> Option<WlSurface> {
        match self {
            KbFocus::Wl(s) => Some(s.clone()),
            #[cfg(feature = "xwayland")]
            KbFocus::X11(x) => x.wl_surface(),
        }
    }
}

impl smithay::utils::IsAlive for KbFocus {
    fn alive(&self) -> bool {
        match self {
            KbFocus::Wl(s) => s.alive(),
            #[cfg(feature = "xwayland")]
            KbFocus::X11(x) => x.alive(),
        }
    }
}

// Required by smithay's data-device / primary-selection / seat (they key the
// selection off the focused client's wl_surface).
impl smithay::wayland::seat::WaylandFocus for KbFocus {
    fn wl_surface(&self) -> Option<std::borrow::Cow<'_, WlSurface>> {
        match self {
            KbFocus::Wl(s) => Some(std::borrow::Cow::Owned(s.clone())),
            #[cfg(feature = "xwayland")]
            KbFocus::X11(x) => smithay::wayland::seat::WaylandFocus::wl_surface(x),
        }
    }
}

impl smithay::input::keyboard::KeyboardTarget<State> for KbFocus {
    fn enter(&self, seat: &Seat<State>, data: &mut State,
        keys: Vec<smithay::input::keyboard::KeysymHandle<'_>>, serial: smithay::utils::Serial) {
        match self {
            KbFocus::Wl(s)  => smithay::input::keyboard::KeyboardTarget::enter(s, seat, data, keys, serial),
            #[cfg(feature = "xwayland")]
            KbFocus::X11(x) => smithay::input::keyboard::KeyboardTarget::enter(x, seat, data, keys, serial),
        }
    }
    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: smithay::utils::Serial) {
        match self {
            KbFocus::Wl(s)  => smithay::input::keyboard::KeyboardTarget::leave(s, seat, data, serial),
            #[cfg(feature = "xwayland")]
            KbFocus::X11(x) => smithay::input::keyboard::KeyboardTarget::leave(x, seat, data, serial),
        }
    }
    fn key(&self, seat: &Seat<State>, data: &mut State,
        key: smithay::input::keyboard::KeysymHandle<'_>, state: smithay::backend::input::KeyState,
        serial: smithay::utils::Serial, time: u32) {
        match self {
            KbFocus::Wl(s)  => smithay::input::keyboard::KeyboardTarget::key(s, seat, data, key, state, serial, time),
            #[cfg(feature = "xwayland")]
            KbFocus::X11(x) => smithay::input::keyboard::KeyboardTarget::key(x, seat, data, key, state, serial, time),
        }
    }
    fn modifiers(&self, seat: &Seat<State>, data: &mut State,
        modifiers: smithay::input::keyboard::ModifiersState, serial: smithay::utils::Serial) {
        match self {
            KbFocus::Wl(s)  => smithay::input::keyboard::KeyboardTarget::modifiers(s, seat, data, modifiers, serial),
            #[cfg(feature = "xwayland")]
            KbFocus::X11(x) => smithay::input::keyboard::KeyboardTarget::modifiers(x, seat, data, modifiers, serial),
        }
    }
}

impl SeatHandler for State {
    type KeyboardFocus = KbFocus;
    type PointerFocus  = WlSurface;
    type TouchFocus    = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> { &mut self.seat_state }
    // Point the clipboard / primary selection at the newly-focused client. The
    // wl_data_device selection is only offered to the focused client, so without
    // this NO app can read or set the clipboard via wl_data_device (alacritty,
    // GTK, Qt…). It went unnoticed because wl-copy/wl-paste use the separate
    // wlr-data-control protocol, which doesn't need focus. The X11 bridge in
    // src/xwayland.rs depends on this too.
    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&KbFocus>) {
        use smithay::reexports::wayland_server::Resource as _;
        let client = focused.and_then(|f| f.wl_surface()).and_then(|s| s.client());
        smithay::wayland::selection::data_device::set_data_device_focus(
            &self.display_handle, seat, client.clone(),
        );
        smithay::wayland::selection::primary_selection::set_primary_focus(
            &self.display_handle, seat, client,
        );
    }
    // (tablet tool cursor images go through TabletSeatHandler, impl'd below —
    // required by cursor-shape-v1's device dispatch.)
    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        // The focused client asked for a cursor shape/surface; the backend reads
        // this each frame. A new shape changes the rendered pixels, so redraw.
        self.cursor_status = image;
        self.pending_redraw = true;
    }
}

// Tablets aren't specially supported; the default no-op tool-image handler is
// enough. Required because cursor-shape-v1 also sets tablet tool cursors.
impl smithay::wayland::tablet_manager::TabletSeatHandler for State {}

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

/// Pointer constraints. A game in mouselook asks to lock the cursor in place
/// (or confine it to a region) and reads raw deltas from zwp_relative_pointer
/// instead of chasing absolute positions. Without these protocols a game can
/// only warp the cursor back to centre every frame to fake it — which is what
/// showed up as the pointer teleporting.
impl smithay::wayland::pointer_constraints::PointerConstraintsHandler for State {
    // Arming happens in the motion path, once the pointer is known to be
    // inside the constraint region.
    fn new_constraint(
        &mut self,
        _surface: &WlSurface,
        _pointer: &smithay::input::pointer::PointerHandle<Self>,
    ) {
    }

    fn remove_constraint(
        &mut self,
        _surface: &WlSurface,
        _pointer: &smithay::input::pointer::PointerHandle<Self>,
    ) {
    }

    /// Where the client wants the cursor left when the lock lifts, so it
    /// doesn't reappear wherever it happened to be frozen.
    fn cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        pointer: &smithay::input::pointer::PointerHandle<Self>,
        location: Point<f64, Logical>,
    ) {
        let active = smithay::wayland::pointer_constraints::with_pointer_constraint(
            surface, pointer, |c| c.map(|c| c.is_active()).unwrap_or(false),
        );
        if !active { return; }
        if let Some((_, origin)) = self.surface_under(self.pointer_location) {
            self.pointer_location = origin + location;
        }
    }
}

smithay::delegate_dispatch2!(State);

impl smithay::wayland::xdg_activation::XdgActivationHandler for State {
    fn activation_state(&mut self) -> &mut smithay::wayland::xdg_activation::XdgActivationState {
        &mut self.xdg_activation_state
    }

    /// Honour a focus request only while its token is fresh (the user just
    /// did something in the requesting app) — stale tokens are how focus
    /// stealing happens. The target comes forward wherever it lives: another
    /// desk, behind a tab, or parked on the shelf.
    fn request_activation(
        &mut self,
        token: smithay::wayland::xdg_activation::XdgActivationToken,
        token_data: smithay::wayland::xdg_activation::XdgActivationTokenData,
        surface: WlSurface,
    ) {
        self.xdg_activation_state.remove_token(&token);
        if token_data.timestamp.elapsed() > std::time::Duration::from_secs(10) { return; }
        let is = |w: &Window| w.wl_surface().is_some_and(|s| *s == surface);
        if let Some(w) = self.workspaces.scratchpad.iter().find(|w| is(w)).cloned() {
            self.stage_swap(&w);
            return;
        }
        let Some(w) = self.workspaces.iter()
            .flat_map(|ws| ws.windows().into_iter()
                .chain(ws.stacks.iter().flat_map(|s| s.members.clone())))
            .find(|w| is(w))
        else { return };
        if let Some(id) = self.workspaces.find_workspace(&w) {
            if id != self.workspaces.active_id() { self.switch_workspace(id); }
        }
        if self.workspaces.active_ref().stack_of(&w).is_some() {
            self.activate_tab(&w);
        }
        self.focus_window(&w);
        self.space.raise_element(&w, true);
        self.pending_redraw = true;
    }
}
