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

use smithay::backend::renderer::{
    element::{
        Kind,
        memory::MemoryRenderBufferRenderElement,
        surface::WaylandSurfaceRenderElement,
        utils::RescaleRenderElement,
    },
    gles::{GlesPixelProgram, GlesTexProgram, Uniform, UniformName, UniformType, element::PixelShaderElement},
};

// Chrome (tab strips + stage shelf) for the nested backend — the same
// primitives the udev session draws, minus its animations and blur.
smithay::backend::renderer::element::render_elements! {
    ChromeElems<=GlesRenderer>;
    Pixel=PixelShaderElement,
    Memory=MemoryRenderBufferRenderElement<GlesRenderer>,
    Thumb=RescaleRenderElement<crate::render::RoundedElement>,
}

fn chrome_elements(
    items: &[crate::chrome::Item],
    off: (f64, f64),
    renderer: &mut GlesRenderer,
    border: &GlesPixelProgram,
    rounded: &GlesTexProgram,
    text: &mut crate::text::TextCache,
    out: &mut Vec<ChromeElems>,
) {
    use crate::chrome::Item;
    let scale = smithay::utils::Scale::from(1.0);
    let ri = |r: &Rectangle<f64, smithay::utils::Logical>| Rectangle::<i32, smithay::utils::Logical>::new(
        ((r.loc.x + off.0).round() as i32, (r.loc.y + off.1).round() as i32).into(),
        (r.size.w.round().max(1.0) as i32, r.size.h.round().max(1.0) as i32).into(),
    );
    for item in items.iter().rev() {
        match item {
            Item::Rect { rect, radius, color } | Item::Ring { rect, radius, color, .. } => {
                let thickness = match item {
                    Item::Ring { thickness, .. } => *thickness,
                    _ => rect.size.w.max(rect.size.h) as f32,
                };
                out.push(ChromeElems::Pixel(PixelShaderElement::new(
                    border.clone(), ri(rect), None, 1.0,
                    vec![Uniform::new("color", *color), Uniform::new("radius", *radius),
                         Uniform::new("thickness", thickness)],
                    Kind::Unspecified,
                )));
            }
            Item::Text { x, cy, max_w, text: s, px, color, center } => {
                let Some((buf, (w, h))) = text.get(s, *px, *color, *max_w as f32) else { continue };
                let lx = if *center { x + (max_w - w as f64).max(0.0) / 2.0 } else { *x };
                let loc = smithay::utils::Point::<f64, smithay::utils::Physical>::from((
                    (lx + off.0).round(), (cy + off.1 - h as f64 / 2.0).round()));
                if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(
                    renderer, loc, buf, Some(color[3]), None, None, Kind::Unspecified)
                {
                    out.push(ChromeElems::Memory(e));
                }
            }
            Item::Thumb { window, rect, radius, alpha } => {
                let c = window.geometry().size;
                if c.w <= 0 || c.h <= 0 { continue; }
                let cell = ri(rect);
                let loc = (cell.loc - window.geometry().loc).to_physical_precise_round(scale);
                let surfs: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                    smithay::backend::renderer::element::AsRenderElements::<GlesRenderer>::render_elements(
                        window, renderer, loc, scale, *alpha);
                let m = smithay::utils::Scale { x: cell.size.w as f64 / c.w as f64, y: cell.size.h as f64 / c.h as f64 };
                let r = *radius;
                for e in surfs {
                    out.push(ChromeElems::Thumb(RescaleRenderElement::from_element(
                        crate::render::RoundedElement::new(e, rounded.clone(), r),
                        cell.loc.to_physical_precise_round(scale), m)));
                }
            }
        }
    }
}

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
    let _ = smithay::wayland::cursor_shape::CursorShapeManagerState::new::<State>(&dh);
    let _ = smithay::wayland::virtual_keyboard::VirtualKeyboardManagerState::new::<State, _>(&dh, |_client| true);
    let output_manager_state = OutputManagerState::new_with_xdg_output::<State>(&dh);
    let layer_shell_state    = smithay::wayland::shell::wlr_layer::WlrLayerShellState::new::<State>(&dh);
    let session_lock_state   = smithay::wayland::session_lock::SessionLockManagerState::new::<State, _>(&dh, |_| true);
    let primary_selection_state = smithay::wayland::selection::primary_selection::PrimarySelectionState::new::<State>(&dh);
    let data_control_state   = smithay::wayland::selection::wlr_data_control::DataControlState::new::<State, _>(
        &dh, Some(&primary_selection_state), |_| true);
    // Pointer lock + raw deltas. Games doing mouselook need these two:
    // without them a client can only ever see absolute positions, so it
    // warps the cursor back to centre every frame — which is what read as
    // the pointer "teleporting".
    let pointer_constraints_state =
        smithay::wayland::pointer_constraints::PointerConstraintsState::new::<State>(&dh);
    let _relative_pointer_state =
        smithay::wayland::relative_pointer::RelativePointerManagerState::new::<State>(&dh);
    let _ = &pointer_constraints_state;
    let idle_inhibit_state   = smithay::wayland::idle_inhibit::IdleInhibitManagerState::new::<State>(&dh);
    let xdg_decoration_state = smithay::wayland::shell::xdg::decoration::XdgDecorationState::new::<State>(&dh);
    let viewporter_state     = smithay::wayland::viewporter::ViewporterState::new::<State>(&dh);
    let fractional_scale_manager_state =
        smithay::wayland::fractional_scale::FractionalScaleManagerState::new::<State>(&dh);
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
            crate::config::Config { keybinds: Default::default(), keybinds_pretty: Default::default(), theme: Default::default(), idle_lock_secs: 0, idle_screen_off_secs: 0, idle_screensaver_secs: 0, kb_layout: "us".into(), kb_variant: String::new(), kb_options: String::new(), repeat_delay: 200, repeat_rate: 25, natural_scroll: None, tap_to_click: None, accel_speed: None, disable_while_typing: None, focus_follows_mouse: false, outputs: Vec::new(), window_rules: Vec::new() }
        });

    #[cfg(feature = "xwayland")]
    let xwayland_shell_state =
        smithay::wayland::xwayland_shell::XWaylandShellState::new::<State>(&dh);

    let mut state = State {
        display_handle: dh.clone(),
        pending_output_modes: false,
        #[cfg(feature = "xwayland")]
        xwayland_shell_state,
        #[cfg(feature = "xwayland")]
        xwm: None,
        #[cfg(feature = "xwayland")]
        xdisplay: None,
        #[cfg(feature = "udev")]
        udev: None,
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
        data_control_state,
        idle_inhibit_state,
        xdg_activation_state: smithay::wayland::xdg_activation::XdgActivationState::new::<State>(&dh),
        idle_inhibitors: Default::default(),
        xdg_decoration_state,
        viewporter_state,
        fractional_scale_manager_state,
        seat,
        lock_pending: None,
        locked: false,
        lock_surfaces: Vec::new(),
        space,
        popups: PopupManager::default(),
        workspaces: crate::workspaces::Workspaces::new(),
        window_titles: Default::default(),
        rule_checked: Default::default(),
        drag: None,
        drag_release: None,
        swipe: None,
        touch: None,
        touch_points: Default::default(),
        touch_gesture: None,
        overview: false,
        overview_t: std::time::Instant::now(),
        screenshot: None,
        pending_screencopy: Vec::new(),
        wallpaper_gen: 0,
        vlock: false,
        vlock_input: String::new(),
        vlock_fail: None,
        last_zone: Vec::new(),
        last_activity: std::time::Instant::now(),
        auto_lock_fired: false,
        screen_off: false,
        screensaver_child: None,
        screensaver: None,
        screensaver_fired: false,
        screensaver_t: None,
        screensaver_closing: None,
        open_anims: Vec::new(),
        ws_anim: None,
        geo_anims: Vec::new(),
        fullscreen_anim: None,
        startup_t: None,
        closing: Vec::new(),
        last_geos: std::collections::HashMap::new(),
        tile_geos: std::collections::HashMap::new(),
        drop_preview: None,
        chrome: Default::default(),
        touch_chrome_drag: false,
        ws_anim_output: String::new(),
        dirty_outputs: Vec::new(),
        config,
        pointer_location: (0.0, 0.0).into(),
        cursor_status: smithay::input::pointer::CursorImageStatus::default_named(),
        pending_dmabuf_imports: Vec::new(),
        pending_ipc_events: Vec::new(),
        pending_redraw: true,
        quit_requested: false,
    };

    let pointer = state.seat.add_pointer();
    let _ = state.seat.add_touch();

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
    state.sync_outputs();

    let mut ipc = crate::ipc::Server::bind(&socket_name)
        .context("start IPC server")?;

    let mut clients: Vec<_> = Vec::new();
    let start_time = std::time::Instant::now();
    let mut quit_requested = false;
    let kb_xkb = smithay::input::keyboard::XkbConfig {
        layout: &state.config.kb_layout,
        variant: &state.config.kb_variant,
        options: if state.config.kb_options.is_empty() { None } else { Some(state.config.kb_options.clone()) },
        ..Default::default()
    };
    let keyboard = state.seat.add_keyboard(kb_xkb, state.config.repeat_delay, state.config.repeat_rate)
        .context("add keyboard to seat")?;

    let (border_prog, rounded_prog) = {
        let r = backend.renderer();
        let rounded = r.compile_custom_texture_shader(
            crate::render::ROUNDED_TEX_FRAG,
            &[UniformName::new("size", UniformType::_2f), UniformName::new("radius", UniformType::_1f)],
        ).map_err(|e| anyhow::anyhow!("compile rounded shader: {e:?}"))?;
        let border = r.compile_custom_pixel_shader(
            crate::render::BORDER_FRAG,
            &[UniformName::new("color", UniformType::_4f), UniformName::new("radius", UniformType::_1f),
              UniformName::new("thickness", UniformType::_1f)],
        ).map_err(|e| anyhow::anyhow!("compile border shader: {e:?}"))?;
        (border, rounded)
    };
    let mut text_cache = crate::text::TextCache::default();

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
                    state.update_output_focus();
                    if state.drag.is_some() { state.drag_update(); return; }
                    if state.chrome_motion() { return; }
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
                    let pressed = bstate == smithay::backend::input::ButtonState::Pressed;
                    if !pressed && state.drag.is_some() {
                        if let Some(d) = state.drag.take() { state.drop_dragged(d); }
                        return;
                    }
                    if state.chrome_button(event.button_code(), pressed) { return; }
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

            // Chrome beneath the windows: shelf cards, then tab strips.
            let scene = state.chrome_scene();
            let pal = crate::chrome::Palette::from_theme(&state.config.theme);
            let mut chrome: Vec<ChromeElems> = Vec::new();
            for (wid, info) in &scene.tabs {
                let Some(w) = state.space.elements().find(|w| crate::state::window_id(w) == *wid) else { continue };
                let Some(g) = state.space.element_geometry(w) else { continue };
                let items = crate::chrome::strip_items(info, g.size.w as f64, state.config.theme.radius, &pal, 1.0);
                chrome_elements(&items,
                    (g.loc.x as f64, (g.loc.y - crate::chrome::TAB_H - crate::chrome::TAB_GAP) as f64),
                    renderer, &border_prog, &rounded_prog, &mut text_cache, &mut chrome);
            }
            chrome_elements(&scene.shelf, (0.0, 0.0), renderer, &border_prog, &rounded_prog,
                &mut text_cache, &mut chrome);
            text_cache.sweep();

            let mut frame = renderer
                .render(&mut framebuffer, size, Transform::Flipped180)
                .context("begin frame")?;

            // Mocha base #1E1E2E = (0.117, 0.117, 0.180).
            frame.clear(Color32F::new(0.117, 0.117, 0.180, 1.0), &[damage])
                .context("clear")?;
            draw_render_elements(&mut frame, 1.0, &chrome, &[damage])
                .context("draw chrome")?;
            draw_render_elements(&mut frame, 1.0, &elements, &[damage])
                .context("draw elements")?;
            let _ = frame.finish().context("finish frame")?;

            // Send frame callbacks to every mapped window so they keep drawing.
            for window in state.space.elements().chain(scene.live.iter()) {
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
