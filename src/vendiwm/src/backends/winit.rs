// Winit backend — runs vendiwm as a nested Wayland client inside another
// compositor. Opens a window, renders client surfaces into it, lets you spawn
// Wayland clients against `$WAYLAND_DISPLAY = <our socket>`.

use anyhow::{Context, Result};
use std::sync::Arc;
use smithay::reexports::winit::platform::pump_events::PumpStatus;

use smithay::{
    backend::{
        allocator::dmabuf::Dmabuf,
        input::{
            AbsolutePositionEvent, Event as InputEventTrait, InputEvent, KeyboardKeyEvent,
            PointerAxisEvent, PointerButtonEvent,
        },
        renderer::{
            Color32F, Frame, Renderer, ImportDma, ImportMemWl,
            gles::GlesRenderer,
            utils::draw_render_elements,
        },
        winit::{self, WinitEvent},
    },
    desktop::{PopupManager, Space, Window, space::space_render_elements},
    input::{
        keyboard::FilterResult,
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::wayland_server::{
        Display, ListeningSocket,
        protocol::wl_surface,
    },
    utils::{Rectangle, SERIAL_COUNTER, Transform},
    wayland::{
        compositor::{
            CompositorState, SurfaceAttributes, TraversalAction, with_surface_tree_downward,
        },
        dmabuf::DmabufState,
        output::OutputManagerState,
        seat::WaylandFocus,
        selection::data_device::DataDeviceState,
        shell::xdg::XdgShellState,
        shm::ShmState,
    },
};

use crate::state::{ClientState, State};

pub fn run() -> Result<()> {
    let mut display: Display<State> = Display::new().context("create wayland Display")?;
    let dh = display.handle();

    let (mut backend, mut winit_evloop) = winit::init::<GlesRenderer>()
        .map_err(|e| anyhow::anyhow!("init winit backend: {e:?}"))?;

    // Globals — every protocol we expose to clients.
    let compositor_state     = CompositorState::new::<State>(&dh);
    let xdg_shell_state      = XdgShellState::new::<State>(&dh);
    let shm_state            = ShmState::new::<State>(&dh, backend.renderer().shm_formats());
    let data_device_state    = DataDeviceState::new::<State>(&dh);
    let output_manager_state = OutputManagerState::new_with_xdg_output::<State>(&dh);
    let layer_shell_state    = smithay::wayland::shell::wlr_layer::WlrLayerShellState::new::<State>(&dh);
    let session_lock_state   = smithay::wayland::session_lock::SessionLockManagerState::new::<State, _>(&dh, |_| true);
    let primary_selection_state = smithay::wayland::selection::primary_selection::PrimarySelectionState::new::<State>(&dh);
    let xdg_decoration_state = smithay::wayland::shell::xdg::decoration::XdgDecorationState::new::<State>(&dh);
    let viewporter_state     = smithay::wayland::viewporter::ViewporterState::new::<State>(&dh);
    let mut seat_state       = smithay::input::SeatState::new();
    let seat                 = seat_state.new_wl_seat(&dh, "vendi-seat-0");

    // Set up a wl_output for the winit window — clients use this to size
    // themselves correctly. Mode is sized to the current window.
    let output = Output::new(
        "vendiwm-winit".to_string(),
        PhysicalProperties {
            size:          (0, 0).into(),
            subpixel:      Subpixel::Unknown,
            make:          "vendi".into(),
            model:         "Winit".into(),
            serial_number: "0".into(),
        },
    );
    let _output_global = output.create_global::<State>(&dh);
    // Initial mode = whatever winit reports right now. Real size lands on the
    // first Resized event a frame later — relayout fires then anyway.
    let mode = Mode { size: backend.window_size(), refresh: 60_000 };
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        Some(smithay::output::Scale::Integer(1)),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);

    let mut space: Space<Window> = Space::default();
    space.map_output(&output, (0, 0));

    // linux-dmabuf v3 (GPU buffer sharing — required for alacritty, firefox).
    let dmabuf_formats = backend.renderer().dmabuf_formats();
    let mut dmabuf_state = DmabufState::new();
    let _dmabuf_global = dmabuf_state.create_global::<State>(&dh, dmabuf_formats);

    // Legacy wl_drm binding — Mesa EGL clients need this OR dmabuf v4 to talk
    // to us. Without it alacritty/firefox stay stuck on `libEGL warning: fd -1`.
    match backend.renderer().egl_context().display().bind_wl_display(&dh) {
        Ok(_) => tracing::info!("EGL hardware-acceleration enabled (wl_drm bound)"),
        Err(e) => tracing::warn!(?e, "failed to bind wl_display — EGL clients may not work"),
    }

    let config = crate::config::Config::load()
        .unwrap_or_else(|e| {
            tracing::warn!(?e, "config load failed; using empty keybinds");
            crate::config::Config { keybinds: Default::default(), keybinds_pretty: Default::default(), theme: Default::default() }
        });

    let mut state = State {
        compositor_state,
        xdg_shell_state,
        shm_state,
        seat_state,
        data_device_state,
        dmabuf_state,
        layer_shell_state,
        output_manager_state,
        session_lock_state,
        primary_selection_state,
        xdg_decoration_state,
        viewporter_state,
        seat,
        lock_pending: None,
        locked: false,
        lock_surface: None,
        space,
        popups: PopupManager::default(),
        workspaces: crate::workspaces::Workspaces::new(),
        window_titles: Default::default(),
        rule_checked: Default::default(),
        drag: None,
        drag_release: None,
        swipe: None,
        overview: false,
        overview_t: std::time::Instant::now(),
        screenshot: None,
        pending_screencopy: Vec::new(),
        wallpaper_gen: 0,
        vlock: false,
        vlock_input: String::new(),
        vlock_fail: None,
        last_zone: None,
        open_anims: Vec::new(),
        ws_anim: None,
        geo_anims: Vec::new(),
        closing: Vec::new(),
        last_geos: std::collections::HashMap::new(),
        config,
        pointer_location: (0.0, 0.0).into(),
        pending_dmabuf_imports: Vec::new(),
        pending_ipc_events: Vec::new(),
        pending_redraw: true,
        quit_requested: false,
    };

    let pointer = state.seat.add_pointer();

    // Pick the first free wayland-N name. Bail rather than overwrite an
    // existing compositor's socket.
    let listener = ListeningSocket::bind_auto("vendiwm", 1..=32)
        .context("bind vendiwm wayland socket")?;
    let socket_name = listener
        .socket_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "<unknown>".into());
    tracing::info!(socket = %socket_name, "vendiwm listening — set WAYLAND_DISPLAY to this and spawn a client");

    // IPC socket paired with the wayland socket name.
    let mut ipc = crate::ipc::Server::bind(&socket_name)
        .context("start IPC server")?;

    let mut clients: Vec<_> = Vec::new();
    let start_time = std::time::Instant::now();
    let mut quit_requested = false;
    let keyboard = state.seat.add_keyboard(Default::default(), 200, 25)
        .context("add keyboard to seat")?;

    loop {
        let status = winit_evloop.dispatch_new_events(|event| match event {
            WinitEvent::Resized { .. } => {}
            WinitEvent::Input(event) => match event {
                InputEvent::Keyboard { event } => {
                    // Resolve to a keysym + check modifiers; intercept Super-
                    // chords for our bindings, forward everything else to the
                    // focused client.
                    let key_state = event.state();
                    let action = keyboard.input::<Option<crate::input::Action>, _>(
                        &mut state,
                        event.key_code(),
                        key_state,
                        0.into(), 0,
                        |data, mods, handle| {
                            let sym = handle.modified_sym();
                            crate::input::handle(&data.config, sym.raw(), key_state, mods)
                                .or_else(|| handle.raw_syms().iter().find_map(|s| {
                                    crate::input::handle(&data.config, s.raw(), key_state, mods)
                                }))
                                .map_or(FilterResult::Forward, |a| FilterResult::Intercept(Some(a)))
                        },
                    );
                    if let Some(Some(act)) = action {
                        if state.run_action(act) { quit_requested = true; }
                    }
                }
                InputEvent::PointerMotionAbsolute { event } => {
                    // Winit gives us window-relative coordinates. Scale=1 in
                    // nested mode, so physical == logical numerically.
                    let size = backend.window_size().to_logical(1);
                    let pos  = event.position_transformed(size);
                    state.pointer_location = pos;
                    let under = state.surface_under(pos).map(|(s, p)| (s.into(), p));
                    pointer.motion(&mut state, under, &MotionEvent {
                        location: pos,
                        serial:   SERIAL_COUNTER.next_serial(),
                        time:     InputEventTrait::time_msec(&event),
                    });
                    pointer.frame(&mut state);
                }
                InputEvent::PointerButton { event } => {
                    let bstate = event.state();
                    // Click-to-focus on press.
                    if bstate == smithay::backend::input::ButtonState::Pressed {
                        state.focus_window_at_cursor();
                    }
                    if let Some(button) = event.button_code().into() {
                        pointer.button(&mut state, &ButtonEvent {
                            button,
                            state:  bstate,
                            serial: SERIAL_COUNTER.next_serial(),
                            time:   InputEventTrait::time_msec(&event),
                        });
                    }
                    pointer.frame(&mut state);
                }
                InputEvent::PointerAxis { event } => {
                    use smithay::backend::input::{Axis, AxisSource};
                    let mut frame = AxisFrame::new(InputEventTrait::time_msec(&event))
                        .source(AxisSource::Wheel);
                    if let Some(h) = event.amount(Axis::Horizontal) {
                        frame = frame.value(Axis::Horizontal, h);
                    }
                    if let Some(v) = event.amount(Axis::Vertical) {
                        frame = frame.value(Axis::Vertical, v);
                    }
                    pointer.axis(&mut state, frame);
                    pointer.frame(&mut state);
                }
                _ => {}
            },
            _ => {}
        });

        if let PumpStatus::Exit(_) = status {
            tracing::info!("winit window closed, exiting");
            return Ok(());
        }
        if quit_requested {
            tracing::info!("quit action received, exiting");
            return Ok(());
        }

        // Each frame: sync the output's mode to what winit thinks the window
        // currently is. If it changed, relayout so the client gets reconfigured.
        let size   = backend.window_size();
        let damage = Rectangle::from_size(size);
        let mode   = Mode { size, refresh: 60_000 };
        let last_mode = output.current_mode();
        if last_mode.map(|m| m.size) != Some(size) {
            output.change_current_state(Some(mode), None, None, None);
            output.set_preferred(mode);
            state.relayout();
        }

        // Refresh the space's internal state (window damage, frame timing).
        state.space.refresh();

        // Scope the framebuffer borrow so it's released before backend.submit.
        {
            let (renderer, mut framebuffer) = backend.bind().context("bind framebuffer")?;

            let elements = space_render_elements(
                renderer,
                [&state.space],
                &output,
                1.0,
            ).context("gather space render elements")?;

            let mut frame = renderer
                .render(&mut framebuffer, size, Transform::Flipped180)
                .context("begin frame")?;

            // Mocha base #1E1E2E = (0.117, 0.117, 0.180).
            frame.clear(Color32F::new(0.117, 0.117, 0.180, 1.0), &[damage])
                .context("clear")?;
            draw_render_elements(&mut frame, 1.0, &elements, &[damage])
                .context("draw elements")?;
            let _ = frame.finish().context("finish frame")?;

            // Send frame callbacks to every mapped window so they keep drawing.
            for window in state.space.elements() {
                if let Some(surf) = window.wl_surface() {
                    send_frames_surface_tree(&surf, start_time.elapsed().as_millis() as u32);
                }
            }

            if let Some(stream) = listener.accept().context("accept client")? {
                let client = display.handle()
                    .insert_client(stream, Arc::new(ClientState::default()))
                    .context("insert client")?;
                clients.push(client);
            }

            display.dispatch_clients(&mut state).context("dispatch clients")?;

            // Pump IPC requests once per frame, then push any queued events
            // (window opened/focused/etc.) to subscribed clients.
            ipc.poll(&mut state);
            for event in state.pending_ipc_events.drain(..) {
                ipc.emit(event);
            }

            // Process queued dmabuf imports — the renderer needs &mut access
            // which the dmabuf_imported handler couldn't get.
            let pending: Vec<(Dmabuf, _)> = state.pending_dmabuf_imports.drain(..).collect();
            for (dmabuf, notifier) in pending {
                if renderer.import_dmabuf(&dmabuf, None).is_ok() {
                    let _ = notifier.successful::<State>();
                } else {
                    notifier.failed();
                }
            }

            display.flush_clients().context("flush clients")?;
        }

        backend.submit(Some(&[damage])).context("submit frame")?;
    }
}

fn send_frames_surface_tree(surface: &wl_surface::WlSurface, time_ms: u32) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_, states, &()| {
            for cb in states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .drain(..)
            {
                cb.done(time_ms);
            }
        },
        |_, _, &()| true,
    );
}
