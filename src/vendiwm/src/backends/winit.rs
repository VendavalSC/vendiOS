// Winit backend — runs vendiwm as a nested Wayland client inside another
// compositor. Opens a window, renders client surfaces into it, lets you spawn
// Wayland clients against `$WAYLAND_DISPLAY = <our socket>`.

use anyhow::{Context, Result};
use std::sync::Arc;
use smithay::reexports::winit::platform::pump_events::PumpStatus;

use smithay::{
    backend::{
        allocator::dmabuf::Dmabuf,
        input::{InputEvent, KeyboardKeyEvent},
        renderer::{
            Color32F, Frame, Renderer, ImportDma, ImportMemWl,
            element::{
                Kind,
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            },
            gles::GlesRenderer,
            utils::draw_render_elements,
        },
        winit::{self, WinitEvent},
    },
    input::keyboard::FilterResult,
    reexports::wayland_server::{Display, ListeningSocket, protocol::wl_surface},
    utils::{Rectangle, Transform},
    wayland::{
        compositor::{
            CompositorState, SurfaceAttributes, TraversalAction, with_surface_tree_downward,
        },
        dmabuf::DmabufState,
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
    let compositor_state  = CompositorState::new::<State>(&dh);
    let xdg_shell_state   = XdgShellState::new::<State>(&dh);
    let shm_state         = ShmState::new::<State>(&dh, backend.renderer().shm_formats());
    let data_device_state = DataDeviceState::new::<State>(&dh);
    let mut seat_state    = smithay::input::SeatState::new();
    let seat              = seat_state.new_wl_seat(&dh, "vendi-seat-0");

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

    let mut state = State {
        compositor_state,
        xdg_shell_state,
        shm_state,
        seat_state,
        data_device_state,
        dmabuf_state,
        seat,
        pending_dmabuf_imports: Vec::new(),
    };

    // Pick the first free wayland-N name. Bail rather than overwrite an
    // existing compositor's socket.
    let listener = ListeningSocket::bind_auto("vendiwm", 1..=32)
        .context("bind vendiwm wayland socket")?;
    let socket_name = listener
        .socket_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "<unknown>".into());
    tracing::info!(socket = %socket_name, "vendiwm listening — set WAYLAND_DISPLAY to this and spawn a client");

    let mut clients: Vec<_> = Vec::new();
    let start_time = std::time::Instant::now();
    let keyboard = state.seat.add_keyboard(Default::default(), 200, 25)
        .context("add keyboard to seat")?;

    loop {
        let status = winit_evloop.dispatch_new_events(|event| match event {
            WinitEvent::Resized { .. } => {}
            WinitEvent::Input(event) => match event {
                InputEvent::Keyboard { event } => {
                    keyboard.input::<(), _>(
                        &mut state,
                        event.key_code(),
                        event.state(),
                        0.into(), 0,
                        |_, _, _| FilterResult::Forward,
                    );
                }
                InputEvent::PointerMotionAbsolute { .. } => {
                    // Auto-focus the most recent toplevel for now so keypresses
                    // reach the test client. Proper focus follows once the
                    // input/focus module lands.
                    if let Some(surf) = state.xdg_shell_state.toplevel_surfaces().iter().next().cloned() {
                        let s = surf.wl_surface().clone();
                        keyboard.set_focus(&mut state, Some(s), 0.into());
                    }
                }
                _ => {}
            },
            _ => {}
        });

        if let PumpStatus::Exit(_) = status {
            tracing::info!("winit window closed, exiting");
            return Ok(());
        }

        let size   = backend.window_size();
        let damage = Rectangle::from_size(size);

        // Scope the framebuffer borrow so it's released before backend.submit.
        {
            let (renderer, mut framebuffer) = backend.bind().context("bind framebuffer")?;

            let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = state
                .xdg_shell_state
                .toplevel_surfaces()
                .iter()
                .flat_map(|surf| render_elements_from_surface_tree(
                    renderer,
                    surf.wl_surface(),
                    (0, 0),
                    1.0, 1.0,
                    Kind::Unspecified,
                ))
                .collect();

            let mut frame = renderer
                .render(&mut framebuffer, size, Transform::Flipped180)
                .context("begin frame")?;

            // Mocha base #1E1E2E = (0.117, 0.117, 0.180).
            frame.clear(Color32F::new(0.117, 0.117, 0.180, 1.0), &[damage])
                .context("clear")?;
            draw_render_elements(&mut frame, 1.0, &elements, &[damage])
                .context("draw elements")?;
            let _ = frame.finish().context("finish frame")?;

            for surf in state.xdg_shell_state.toplevel_surfaces() {
                send_frames_surface_tree(surf.wl_surface(), start_time.elapsed().as_millis() as u32);
            }

            if let Some(stream) = listener.accept().context("accept client")? {
                let client = display.handle()
                    .insert_client(stream, Arc::new(ClientState::default()))
                    .context("insert client")?;
                clients.push(client);
            }

            display.dispatch_clients(&mut state).context("dispatch clients")?;

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
