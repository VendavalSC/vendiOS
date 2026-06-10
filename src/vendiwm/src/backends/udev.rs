// Udev backend — vendiwm running as the session compositor.
//
// Talks directly to DRM/KMS for output, libinput for input, libseat for VT
// management + secure DRM/input fd access. This is what runs on boot.
//
// Pipeline:
//   open DRM device → enumerate connectors → pick first connected → create
//   DrmSurface on a matching CRTC → wrap in a DrmCompositor → wire VBlank
//   events into a render_surface() that composes the desktop Space and
//   queues a frame. Wayland clients connect via the listening socket and
//   are dispatched through a calloop Generic source over the Display fd.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use smithay::{
    backend::{
        allocator::{
            Fourcc,
            gbm::{GbmAllocator, GbmBufferFlags},
        },
        drm::{
            DrmDevice, DrmDeviceFd, DrmDeviceNotifier, DrmEvent, DrmNode, NodeType,
            compositor::{DrmCompositor, FrameFlags},
            exporter::gbm::{GbmFramebufferExporter, NodeFilter},
        },
        egl::{EGLContext, EGLDisplay, context::ContextPriority},
        input::{
            AbsolutePositionEvent, Event as InputEventTrait, InputEvent, KeyboardKeyEvent,
            PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
        },
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            Color32F, ImportDma, ImportMemWl,
            element::{
                AsRenderElements, Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                surface::WaylandSurfaceRenderElement,
                utils::{Relocate, RelocateRenderElement, RescaleRenderElement},
            },
            gles::{
                GlesPixelProgram, GlesRenderer, GlesTexProgram, Uniform, UniformName,
                UniformType, element::PixelShaderElement,
            },
        },
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::{UdevBackend, UdevEvent, all_gpus, primary_gpu},
    },
    desktop::layer_map_for_output,
    input::{
        keyboard::FilterResult,
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    output::{Mode as WlMode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::{
            EventLoop, Interest, Mode as CalloopMode, PostAction,
            generic::Generic,
        },
        drm::control::{Device as ControlDevice, ModeTypeFlags, connector, crtc},
        gbm::Device as GbmDevice,
        input::Libinput,
        rustix::fs::OFlags,
        wayland_server::{Display, DisplayHandle},
    },
    utils::{DeviceFd, Transform, SERIAL_COUNTER},
    wayland::{
        compositor::{SurfaceAttributes, TraversalAction, with_surface_tree_downward},
        seat::WaylandFocus,
        socket::ListeningSocketSource,
    },
};

use crate::cursor::Cursor;
use crate::state::{ClientState, State};

// The render-element enum for our output: cursor (memory blit), layer
// surfaces (bar/menu/notifications), windows (rounded + animatable via
// rescale), and shader-drawn border rings. render_frame wants a homogeneous
// slice, so one enum implements RenderElement for all of them.
smithay::backend::renderer::element::render_elements! {
    pub OutputRenderElements<=GlesRenderer>;
    Layer=WaylandSurfaceRenderElement<GlesRenderer>,
    Memory=MemoryRenderBufferRenderElement<GlesRenderer>,
    Window=RelocateRenderElement<RescaleRenderElement<crate::render::RoundedElement>>,
    Pixel=RescaleRenderElement<PixelShaderElement>,
}

type FrameElement = OutputRenderElements;

/// Concrete `DrmCompositor` parameterisation we use. `U=()` means we hand a
/// unit value to `queue_frame` (no per-frame presentation feedback userdata).
type GbmDrmCompositor = DrmCompositor<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    (),
    DrmDeviceFd,
>;

/// Top-level event-loop data — gives every calloop callback &mut access to
/// the wayland state, the udev/DRM bits, and the display handle for socket
/// hand-off.
pub struct UdevApp {
    pub state:          State,
    pub udev:           UdevData,
    pub display_handle: DisplayHandle,
}

pub fn run() -> Result<()> {
    let mut event_loop: EventLoop<UdevApp> = EventLoop::try_new().context("calloop event loop")?;
    let loop_handle = event_loop.handle();

    let display: Display<State> = Display::new().context("create wayland Display")?;
    let display_handle = display.handle();

    // 1. Open seat from logind. notifier reports VT switch / pause / resume.
    let (session, session_notifier) = LibSeatSession::new()
        .context("LibSeatSession::new — is logind reachable?")?;
    let seat_name = session.seat();
    tracing::info!(seat = %seat_name, "acquired libseat session");

    // 2. Pick primary GPU. Prefer the render node so software clients can
    //    share buffers easily.
    let primary_gpu_path = primary_gpu(&seat_name)
        .context("query primary gpu")?
        .or_else(|| all_gpus(&seat_name).ok()?.into_iter().next())
        .ok_or_else(|| anyhow::anyhow!("no GPU found"))?;
    let primary_gpu_node = DrmNode::from_path(&primary_gpu_path)
        .with_context(|| format!("DrmNode::from_path {:?}", primary_gpu_path))?
        .node_with_type(NodeType::Render)
        .and_then(Result::ok)
        .unwrap_or_else(|| DrmNode::from_path(&primary_gpu_path).unwrap());
    tracing::info!(?primary_gpu_node, "selected primary GPU");

    // 3. Libinput — input events.
    let mut libinput_context = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(
        session.clone().into(),
    );
    libinput_context.udev_assign_seat(&seat_name)
        .map_err(|_| anyhow::anyhow!("libinput failed to assign seat {seat_name}"))?;
    let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

    // 4. Udev — watch for GPU hotplug.
    let udev_backend = UdevBackend::new(&seat_name)
        .context("UdevBackend::new")?;

    let mut udev = UdevData {
        seat_name: seat_name.clone(),
        session,
        primary_gpu: primary_gpu_node,
        drm_devices: HashMap::new(),
    };

    // 5. Open primary GPU. This brings up DRM/GBM/EGL/GlesRenderer and
    //    enumerates connectors. The notifier delivers VBlank events.
    let (device_state, drm_notifier) = udev.open_drm_device(&primary_gpu_path)
        .context("open primary GPU")?;
    let shm_formats: Vec<_> = device_state.renderer.shm_formats().collect();
    udev.drm_devices.insert(primary_gpu_node, device_state);

    // 6. Build the wayland State with the renderer's SHM formats. wl_drm
    //    binding for Mesa EGL clients happens next.
    let state = build_state(&display_handle, shm_formats)?;
    let mut app = UdevApp { state, udev, display_handle: display_handle.clone() };

    // 7. Try to bring up the first connected connector. If this fails we
    //    keep going (still useful for VT switch / inputs / log), but you
    //    won't see anything until a working connector shows up.
    let first_crtc = match initial_surface_setup(&mut app, primary_gpu_node) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(?e, "no usable connector at startup; running headless");
            None
        }
    };

    // 8. Bind wl_display to EGL so Mesa clients (alacritty, firefox) get
    //    wl_drm and can hand us GPU-side buffers without falling back to
    //    SHM. Must happen after the renderer is created.
    if let Some(dev) = app.udev.drm_devices.get_mut(&primary_gpu_node) {
        match dev.renderer.egl_context().display().bind_wl_display(&display_handle) {
            Ok(_)  => tracing::info!("EGL hardware-acceleration enabled (wl_drm bound)"),
            Err(e) => tracing::warn!(?e, "failed to bind wl_display — EGL clients may not work"),
        }
    }

    // 9. Calloop sources.
    loop_handle.insert_source(libinput_backend, move |event, _, app: &mut UdevApp| {
        on_libinput_event(event, app);
    }).map_err(|e| anyhow::anyhow!("insert libinput source: {e:?}"))?;

    // VT switch / session pause+resume. We have to release DRM master and
    // suspend libinput when switching away, then re-take them on return —
    // otherwise the kernel can't switch VTs (we hold master) and on resume the
    // input devices stay dead.
    let mut session_libinput = libinput_context.clone();
    loop_handle.insert_source(session_notifier, move |event, _, app: &mut UdevApp| {
        match event {
            SessionEvent::PauseSession => {
                tracing::info!("session paused (VT switched away)");
                session_libinput.suspend();
                for dev in app.udev.drm_devices.values_mut() {
                    dev.drm.pause();
                }
            }
            SessionEvent::ActivateSession => {
                tracing::info!("session activated (VT switched in)");
                if let Err(e) = session_libinput.resume() {
                    tracing::warn!(?e, "libinput resume failed");
                }
                for dev in app.udev.drm_devices.values_mut() {
                    if let Err(e) = dev.drm.activate(false) {
                        tracing::warn!(?e, "drm activate failed");
                    }
                }
                app.state.pending_redraw = true;
            }
        }
    }).map_err(|e| anyhow::anyhow!("insert session source: {e:?}"))?;

    loop_handle.insert_source(udev_backend, move |event, _, app: &mut UdevApp| {
        on_udev_event(event, app);
    }).map_err(|e| anyhow::anyhow!("insert udev source: {e:?}"))?;

    // DRM page-flip / VBlank events drive rendering — one frame on each tick.
    loop_handle.insert_source(drm_notifier, move |event, _, app: &mut UdevApp| {
        match event {
            DrmEvent::VBlank(crtc) => {
                // Acknowledge the just-finished frame, then queue the next.
                if let Some(dev) = app.udev.drm_devices.get_mut(&primary_gpu_node) {
                    if let Some(surf) = dev.surfaces.get_mut(&crtc) {
                        if let Err(e) = surf.compositor.frame_submitted() {
                            tracing::warn!(?e, "frame_submitted");
                        }
                    }
                }
                if let Err(e) = render_surface(app, primary_gpu_node, crtc) {
                    tracing::warn!(?e, "render_surface");
                }
            }
            DrmEvent::Error(e) => tracing::warn!(?e, "drm error"),
        }
    }).map_err(|e| anyhow::anyhow!("insert drm notifier: {e:?}"))?;

    // Wayland client socket: $WAYLAND_DISPLAY for spawned clients.
    let listening = ListeningSocketSource::new_auto()
        .context("bind wayland listening socket")?;
    let socket_name = listening.socket_name().to_string_lossy().into_owned();
    // SAFETY: single-threaded at this point; no other code is reading env.
    unsafe { std::env::set_var("WAYLAND_DISPLAY", &socket_name); }
    tracing::info!(socket = %socket_name, "listening on wayland socket");

    // IPC socket paired with the wayland socket name — vendibar, vendi-ctl
    // and vendi-menu all talk to this. Pumped once per tick below.
    let mut ipc = crate::ipc::Server::bind(&socket_name)
        .context("start IPC server")?;
    let mut dh_for_socket = display_handle.clone();
    loop_handle.insert_source(listening, move |stream, _, _app: &mut UdevApp| {
        if let Err(e) = dh_for_socket.insert_client(stream, Arc::new(ClientState::default())) {
            tracing::warn!(?e, "insert client failed");
        }
    }).map_err(|e| anyhow::anyhow!("insert socket source: {e:?}"))?;

    // Wayland client dispatch — wake on the Display fd, run handlers.
    loop_handle.insert_source(
        Generic::new(display, Interest::READ, CalloopMode::Level),
        |_, display, app: &mut UdevApp| {
            // SAFETY: the Generic source owns the Display for its lifetime
            // and we never drop or move it from inside the callback.
            unsafe {
                let _ = display.get_mut().dispatch_clients(&mut app.state);
            }
            Ok(PostAction::Continue)
        },
    ).map_err(|e| anyhow::anyhow!("insert display source: {e:?}"))?;

    // 10. Kick off the first frame so VBlank-driven rendering can begin.
    if let Some(crtc) = first_crtc {
        loop_handle.insert_idle(move |app| {
            if let Err(e) = render_surface(app, primary_gpu_node, crtc) {
                tracing::warn!(?e, "initial render_surface");
            }
        });
    }

    let mut display_handle_tick = display_handle.clone();
    let loop_signal = event_loop.get_signal();
    tracing::info!("vendiwm udev backend running. Press Ctrl+C to exit.");
    event_loop.run(Duration::from_millis(16), &mut app, move |app| {
        // Per-tick housekeeping: drain dmabuf imports, refresh space damage
        // bookkeeping, flush queued events out to clients.
        app.state.space.refresh();
        app.state.popups.cleanup();

        // Pump the IPC server: deliver queued events, answer requests.
        for ev in app.state.pending_ipc_events.drain(..).collect::<Vec<_>>() {
            ipc.emit(ev);
        }
        ipc.poll(&mut app.state);


        let pending: Vec<_> = app.state.pending_dmabuf_imports.drain(..).collect();
        if !pending.is_empty() {
            if let Some(dev) = app.udev.drm_devices.get_mut(&app.udev.primary_gpu) {
                for (dmabuf, notifier) in pending {
                    if dev.renderer.import_dmabuf(&dmabuf, None).is_ok() {
                        let _ = notifier.successful::<State>();
                    } else {
                        notifier.failed();
                    }
                }
            }
        }

        // Damage-driven render. VBlank already re-renders on its own, but the
        // first frame is empty (no clients yet) so no page-flip → no VBlank →
        // render loop stalls. This restarts it whenever a client commits.
        if app.state.pending_redraw {
            app.state.pending_redraw = false;
            let crtcs: Vec<_> = app.udev.drm_devices.get(&app.udev.primary_gpu)
                .map(|d| d.surfaces.keys().copied().collect())
                .unwrap_or_default();
            for crtc in crtcs {
                if let Err(e) = render_surface(app, app.udev.primary_gpu, crtc) {
                    tracing::trace!(?e, "tick render_surface");
                }
            }
        }

        let _ = display_handle_tick.flush_clients();

        if app.state.quit_requested {
            tracing::info!("quit requested, stopping event loop");
            loop_signal.stop();
        }
    }).context("run event loop")?;

    Ok(())
}

fn build_state(
    dh: &DisplayHandle,
    shm_formats: Vec<smithay::reexports::wayland_server::protocol::wl_shm::Format>,
) -> Result<State> {
    use smithay::wayland::{
        compositor::CompositorState,
        dmabuf::DmabufState,
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        shell::{xdg::XdgShellState, wlr_layer::WlrLayerShellState},
        shm::ShmState,
    };
    use smithay::desktop::{PopupManager, Space};
    use smithay::input::SeatState;

    let compositor_state     = CompositorState::new::<State>(dh);
    let xdg_shell_state      = XdgShellState::new::<State>(dh);
    let shm_state            = ShmState::new::<State>(dh, shm_formats);
    let data_device_state    = DataDeviceState::new::<State>(dh);
    let output_manager_state = OutputManagerState::new_with_xdg_output::<State>(dh);
    let layer_shell_state    = WlrLayerShellState::new::<State>(dh);
    let dmabuf_state         = DmabufState::new();

    let mut seat_state = SeatState::new();
    let mut seat = seat_state.new_wl_seat(dh, "vendi-seat-0");
    let _ = seat.add_keyboard(Default::default(), 200, 25)
        .context("add keyboard to seat")?;
    let _ = seat.add_pointer();

    let config = crate::config::Config::load().unwrap_or_else(|e| {
        tracing::warn!(?e, "config load failed; using empty keybinds");
        crate::config::Config { keybinds: Default::default(), theme: Default::default() }
    });

    Ok(State {
        compositor_state,
        xdg_shell_state,
        shm_state,
        seat_state,
        data_device_state,
        dmabuf_state,
        layer_shell_state,
        output_manager_state,
        seat,
        space:                  Space::default(),
        popups:                 PopupManager::default(),
        workspaces:             crate::workspaces::Workspaces::new(),
        window_titles:          Default::default(),
        drag:                   None,
        swipe:                  None,
        last_zone:              None,
        open_anims:             Vec::new(),
        ws_anim:                None,
        move_anims:             Vec::new(),
        config,
        pointer_location:       (0.0, 0.0).into(),
        pending_dmabuf_imports: Vec::new(),
        pending_ipc_events:     Vec::new(),
        pending_redraw:         true,
        quit_requested:         false,
    })
}

// ── runtime state ─────────────────────────────────────────────────────────────

pub struct UdevData {
    pub seat_name:    String,
    pub session:      LibSeatSession,
    pub primary_gpu:  DrmNode,
    pub drm_devices:  HashMap<DrmNode, DeviceState>,
}

pub struct DeviceState {
    pub drm:        DrmDevice,
    pub gbm:        GbmDevice<DrmDeviceFd>,
    pub renderer:   GlesRenderer,
    pub gpu_path:   PathBuf,
    pub connectors: Vec<connector::Info>,
    pub surfaces:   HashMap<crtc::Handle, SurfaceState>,
    /// XCursor-backed pointer image. Cloned each frame into the render list.
    pub cursor:     Cursor,
    /// Rounded-corner texture shader (applied per window element).
    pub rounded_prog: GlesTexProgram,
    /// Rounded border-ring pixel shader.
    pub border_prog:  GlesPixelProgram,
}

pub struct SurfaceState {
    pub output:     Output,
    pub compositor: GbmDrmCompositor,
    /// Output-sized wallpaper (user image or the built-in gradient).
    pub wallpaper:  MemoryRenderBuffer,
}

impl UdevData {
    fn open_drm_device(&mut self, path: &PathBuf) -> Result<(DeviceState, DrmDeviceNotifier)> {
        // 1. Open the device fd via libseat (rev'd up with DRM master).
        let fd = self.session.open(path,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
        ).map_err(|e| anyhow::anyhow!("session.open {path:?}: {e:?}"))?;
        let device_fd = DrmDeviceFd::new(DeviceFd::from(fd));

        // 2. DrmDevice — atomic KMS in modern mode.
        let (drm, notifier) = DrmDevice::new(device_fd.clone(), true)
            .map_err(|e| anyhow::anyhow!("DrmDevice::new: {e:?}"))?;

        // 3. GbmDevice on the same fd — used for buffer allocation that
        //    DRM scanout + EGL can consume.
        let gbm = GbmDevice::new(device_fd)
            .map_err(|e| anyhow::anyhow!("GbmDevice::new: {e:?}"))?;

        // 4. EGL on top of GBM, then a GlesRenderer. SAFETY: the EGLDisplay is
        //    fresh and we don't already have a current context on this thread.
        let egl_display = unsafe { EGLDisplay::new(gbm.clone())
            .map_err(|e| anyhow::anyhow!("EGLDisplay::new: {e:?}"))? };
        let egl_context = EGLContext::new_with_priority(&egl_display, ContextPriority::High)
            .map_err(|e| anyhow::anyhow!("EGLContext::new: {e:?}"))?;
        let mut renderer = unsafe { GlesRenderer::new(egl_context) }
            .map_err(|e| anyhow::anyhow!("GlesRenderer::new: {e:?}"))?;

        // Premium pipeline shaders: rounded window corners + border rings.
        let rounded_prog = renderer.compile_custom_texture_shader(
            crate::render::ROUNDED_TEX_FRAG,
            &[
                UniformName::new("size",   UniformType::_2f),
                UniformName::new("radius", UniformType::_1f),
            ],
        ).map_err(|e| anyhow::anyhow!("compile rounded shader: {e:?}"))?;
        let border_prog = renderer.compile_custom_pixel_shader(
            crate::render::BORDER_FRAG,
            &[
                UniformName::new("color",     UniformType::_4f),
                UniformName::new("radius",    UniformType::_1f),
                UniformName::new("thickness", UniformType::_1f),
            ],
        ).map_err(|e| anyhow::anyhow!("compile border shader: {e:?}"))?;

        // 5. Enumerate connectors so we can log + later create surfaces.
        let resources = drm.resource_handles()
            .map_err(|e| anyhow::anyhow!("resource_handles: {e:?}"))?;
        let mut connectors = Vec::new();
        for c in resources.connectors() {
            if let Ok(info) = drm.get_connector(*c, true) {
                tracing::info!(
                    name  = %format!("{:?}-{}", info.interface(), info.interface_id()),
                    status = ?info.state(),
                    modes  = info.modes().len(),
                    "DRM connector"
                );
                connectors.push(info);
            }
        }

        let dev = DeviceState {
            drm,
            gbm,
            renderer,
            gpu_path: path.clone(),
            connectors,
            surfaces: HashMap::new(),
            cursor:   Cursor::load(),
            rounded_prog,
            border_prog,
        };
        Ok((dev, notifier))
    }
}

// ── surface setup ─────────────────────────────────────────────────────────────

/// Bring up the first connected connector on the given device: pick a mode,
/// find a usable CRTC, create a DrmSurface + DrmCompositor + a smithay Output,
/// and wire the Output into the Space at (0,0). Returns the CRTC handle so
/// the caller can kick the initial frame.
fn initial_surface_setup(app: &mut UdevApp, node: DrmNode) -> Result<Option<crtc::Handle>> {
    let UdevApp { state, udev, display_handle } = app;
    let device = udev.drm_devices.get_mut(&node)
        .ok_or_else(|| anyhow::anyhow!("device not found: {node:?}"))?;

    // Pick the first connector that's actually plugged in.
    let connector = device.connectors.iter()
        .find(|c| c.state() == connector::State::Connected)
        .cloned();
    let Some(connector) = connector else {
        tracing::warn!("no connected connector");
        return Ok(None);
    };

    // Preferred mode if any, else the first one.
    let mode_idx = connector.modes().iter()
        .position(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
        .unwrap_or(0);
    let drm_mode = *connector.modes().get(mode_idx)
        .ok_or_else(|| anyhow::anyhow!("connector has no modes"))?;

    // Find a CRTC reachable by one of this connector's encoders.
    let resources = device.drm.resource_handles()
        .map_err(|e| anyhow::anyhow!("resource_handles: {e:?}"))?;
    let mut chosen_crtc: Option<crtc::Handle> = None;
    'outer: for enc_handle in connector.encoders() {
        let Ok(enc) = device.drm.get_encoder(*enc_handle) else { continue };
        for crtc in resources.filter_crtcs(enc.possible_crtcs()) {
            // First match wins — we only drive one output for now.
            chosen_crtc = Some(crtc);
            break 'outer;
        }
    }
    let Some(crtc) = chosen_crtc else {
        tracing::warn!("no usable CRTC for connector");
        return Ok(None);
    };

    let planes = device.drm.planes(&crtc)
        .map_err(|e| anyhow::anyhow!("planes: {e:?}"))?;

    // Hand the CRTC the connector + mode. This is the moment KMS commits
    // happen — failure here means DRM master / atomic-modeset issues.
    let drm_surface = device.drm.create_surface(crtc, drm_mode, &[connector.handle()])
        .map_err(|e| anyhow::anyhow!("create_surface: {e:?}"))?;

    // smithay Output mirrors the DRM mode so layout knows about size + refresh.
    let output_name = format!("{:?}-{}", connector.interface(), connector.interface_id());
    let (phys_w, phys_h) = connector.size().unwrap_or((0, 0));
    let output = Output::new(
        output_name.clone(),
        PhysicalProperties {
            size:          (phys_w as i32, phys_h as i32).into(),
            subpixel:      Subpixel::Unknown,
            make:          "vendi".into(),
            model:         "DRM".into(),
            serial_number: "0".into(),
        },
    );
    let wl_mode = WlMode::from(drm_mode);
    output.change_current_state(
        Some(wl_mode),
        Some(Transform::Normal),
        Some(Scale::Integer(1)),
        Some((0, 0).into()),
    );
    output.set_preferred(wl_mode);
    let _global = output.create_global::<State>(display_handle);
    state.space.map_output(&output, (0, 0));

    // Start the pointer at the centre of the screen so the cursor is visible
    // before the user has nudged the mouse.
    let mode_size = drm_mode.size();
    state.pointer_location = (mode_size.0 as f64 / 2.0, mode_size.1 as f64 / 2.0).into();

    // DrmCompositor wires the renderer to scanout. Cursor size 64×64 is the
    // standard everyone supports.
    let allocator = GbmAllocator::new(
        device.gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    let exporter = GbmFramebufferExporter::new(device.gbm.clone(), NodeFilter::None);
    let color_formats = [Fourcc::Abgr8888, Fourcc::Argb8888];
    let renderer_formats = device.renderer.egl_context().dmabuf_render_formats().clone();

    let compositor = DrmCompositor::new(
        &output,
        drm_surface,
        Some(planes),
        allocator,
        exporter,
        color_formats,
        renderer_formats,
        (64u32, 64u32).into(),
        Some(device.gbm.clone()),
    ).map_err(|e| anyhow::anyhow!("DrmCompositor::new: {e:?}"))?;

    tracing::info!(
        crtc = ?crtc,
        connector = %output_name,
        mode = ?(drm_mode.size(), drm_mode.vrefresh()),
        "DRM output up",
    );

    let wallpaper = crate::render::wallpaper_buffer(
        mode_size.0 as i32,
        mode_size.1 as i32,
        state.config.theme.wallpaper.as_deref(),
    );

    device.surfaces.insert(crtc, SurfaceState { output, compositor, wallpaper });
    state.relayout();
    Ok(Some(crtc))
}

/// Render one frame for `crtc` on `node`. Gathers elements from the space,
/// asks the DrmCompositor to compose them, and queues the frame for the next
/// VBlank.
fn render_surface(app: &mut UdevApp, node: DrmNode, crtc: crtc::Handle) -> Result<()> {
    let UdevApp { state, udev, .. } = app;
    let device = udev.drm_devices.get_mut(&node)
        .ok_or_else(|| anyhow::anyhow!("device not found"))?;

    // Split-borrow: hold renderer, surface, and cursor mutably at the same
    // time. They're distinct fields of DeviceState, so this is safe.
    let renderer = &mut device.renderer;
    let cursor   = &device.cursor;
    let rounded_prog = device.rounded_prog.clone();
    let border_prog  = device.border_prog.clone();
    let surface  = device.surfaces.get_mut(&crtc)
        .ok_or_else(|| anyhow::anyhow!("surface not found"))?;

    let scale = smithay::utils::Scale::from(1.0_f64);

    // ── animation clocks ────────────────────────────────────────────────────
    // Open: fade + scale-in per window. Workspace switch: the whole desk
    // fades + settles. Eased with cubic ease-out; while anything is in
    // flight we keep pending_redraw set so the loop renders every tick.
    const OPEN_MS: f32 = 220.0;
    const WS_MS:   f32 = 180.0;
    const MOVE_MS: f32 = 200.0;
    fn ease_out(t: f32) -> f32 { 1.0 - (1.0 - t).powi(3) }
    let now = std::time::Instant::now();
    state.open_anims.retain(|(w, t)| {
        smithay::utils::IsAlive::alive(w)
            && (now.duration_since(*t).as_secs_f32() * 1000.0) < OPEN_MS
    });
    state.move_anims.retain(|(w, _, t)| {
        smithay::utils::IsAlive::alive(w)
            && (now.duration_since(*t).as_secs_f32() * 1000.0) < MOVE_MS
    });
    let ws_progress = state.ws_anim.map(|t| now.duration_since(t).as_secs_f32() * 1000.0 / WS_MS);
    if ws_progress.map(|p| p >= 1.0).unwrap_or(false) {
        state.ws_anim = None;
    }
    let (ws_alpha, ws_scale) = match ws_progress.filter(|p| *p < 1.0) {
        Some(p) => { let e = ease_out(p); (0.25 + 0.75 * e, 0.97 + 0.03 * e as f64) }
        None    => (1.0, 1.0),
    };
    let anims_active = state.ws_anim.is_some()
        || !state.open_anims.is_empty()
        || !state.move_anims.is_empty();
    let theme = state.config.theme.clone();
    let mut upper_layer_elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = Vec::new();
    let mut lower_layer_elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = Vec::new();
    // A fullscreen window hides the Top layer (the bar) — only Overlay
    // surfaces (e.g. a lock screen) stay above it, per the wlr spec.
    let fullscreen_active = state.workspaces.active_ref().fullscreen.is_some();
    {
        let layer_map = layer_map_for_output(&surface.output);
        // `layer_geometry` returns location relative to the output; we feed it
        // to render_elements in physical px so the surface lands where the
        // protocol said it should.
        let upper_layers: Vec<_> = if fullscreen_active {
            layer_map.layers_on(smithay::wayland::shell::wlr_layer::Layer::Overlay).collect()
        } else {
            layer_map.layers_on(smithay::wayland::shell::wlr_layer::Layer::Overlay)
                .chain(layer_map.layers_on(smithay::wayland::shell::wlr_layer::Layer::Top))
                .collect()
        };
        for layer in upper_layers {
            let geo = match layer_map.layer_geometry(layer) { Some(g) => g, None => continue };
            let phys_loc = geo.loc.to_physical_precise_round(scale);
            upper_layer_elems.extend(
                smithay::backend::renderer::element::AsRenderElements::<GlesRenderer>::render_elements::<WaylandSurfaceRenderElement<GlesRenderer>>(
                    layer, renderer, phys_loc, scale, 1.0,
                ),
            );
        }
        for layer in layer_map.layers_on(smithay::wayland::shell::wlr_layer::Layer::Bottom).chain(layer_map.layers_on(smithay::wayland::shell::wlr_layer::Layer::Background)) {
            let geo = match layer_map.layer_geometry(layer) { Some(g) => g, None => continue };
            let phys_loc = geo.loc.to_physical_precise_round(scale);
            lower_layer_elems.extend(
                smithay::backend::renderer::element::AsRenderElements::<GlesRenderer>::render_elements::<WaylandSurfaceRenderElement<GlesRenderer>>(
                    layer, renderer, phys_loc, scale, 1.0,
                ),
            );
        }
    }

    let mut elements: Vec<FrameElement> =
        Vec::with_capacity(upper_layer_elems.len() + lower_layer_elems.len() + 16);

    // Cursor first — render_frame treats `elements` as front-to-back, so
    // index 0 is drawn on top of everything else.
    let pointer_phys = smithay::utils::Point::<f64, smithay::utils::Physical>::from((
        state.pointer_location.x - cursor.hotspot.0 as f64,
        state.pointer_location.y - cursor.hotspot.1 as f64,
    ));
    if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        pointer_phys,
        &cursor.buffer,
        None,
        None,
        None,
        Kind::Cursor,
    ) {
        elements.push(OutputRenderElements::Memory(elem));
    }

    // Upper layers (Top/Overlay) → above windows but below the cursor.
    elements.extend(upper_layer_elems.into_iter().map(OutputRenderElements::Layer));

    // Windows + borders, topmost first. Each window's surfaces go through
    // the rounded-corner shader; its border is an SDF ring drawn just above
    // its own edge; both share the window's animation transform (fade +
    // scale around the center, glide between tile slots).
    let border_w = theme.border;
    let focused_surf = state.seat.get_keyboard().and_then(|k| k.current_focus());
    let fullscreen = state.workspaces.active_ref().fullscreen.clone();
    let stacked: Vec<_> = state.space.elements().cloned().collect();
    for window in stacked.iter().rev() {
        let Some(geo) = state.space.element_geometry(window) else { continue };

        // Per-window open animation on top of the workspace-switch one.
        let open_e = state.open_anims.iter()
            .find(|(w, _)| w == window)
            .map(|(_, t)| ease_out((now.duration_since(*t).as_secs_f32() * 1000.0 / OPEN_MS).min(1.0)));
        let alpha = ws_alpha * open_e.unwrap_or(1.0);
        let scale_anim: f64 = ws_scale * open_e.map(|e| 0.92 + 0.08 * e as f64).unwrap_or(1.0);

        // Tile-move glide: offset from the old slot toward the new one.
        let move_off = state.move_anims.iter()
            .find(|(w, _, _)| w == window)
            .map(|(_, from, t)| {
                let e = ease_out((now.duration_since(*t).as_secs_f32() * 1000.0 / MOVE_MS).min(1.0));
                let dx = ((from.x - geo.loc.x) as f32 * (1.0 - e)).round() as i32;
                let dy = ((from.y - geo.loc.y) as f32 * (1.0 - e)).round() as i32;
                (dx, dy)
            })
            .unwrap_or((0, 0));

        let center = smithay::utils::Point::<i32, smithay::utils::Physical>::from((
            geo.loc.x + move_off.0 + geo.size.w / 2,
            geo.loc.y + move_off.1 + geo.size.h / 2,
        ));

        let is_fullscreen = fullscreen.as_ref() == Some(window);
        let radius = if is_fullscreen { 0.0 } else { theme.radius };

        // Border ring (skip on fullscreen).
        if !is_fullscreen {
            let win_surf = window.wl_surface();
            let focused = matches!((&focused_surf, &win_surf), (Some(f), Some(s)) if **s == *f);
            let c = if focused { theme.accent } else { theme.inactive };
            let area = smithay::utils::Rectangle::<i32, smithay::utils::Logical>::new(
                (geo.loc.x + move_off.0 - border_w, geo.loc.y + move_off.1 - border_w).into(),
                (geo.size.w + border_w * 2, geo.size.h + border_w * 2).into(),
            );
            let ring = PixelShaderElement::new(
                border_prog.clone(),
                area,
                None,
                alpha,
                vec![
                    Uniform::new("color", c),
                    Uniform::new("radius", radius + border_w as f32),
                    Uniform::new("thickness", border_w as f32),
                ],
                Kind::Unspecified,
            );
            elements.push(OutputRenderElements::Pixel(
                RescaleRenderElement::from_element(ring, center, scale_anim),
            ));
        }

        // Window content (toplevel + subsurfaces + popups), rounded.
        let render_loc = (geo.loc - window.geometry().loc).to_physical_precise_round(scale);
        let surfaces: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
            window.render_elements(renderer, render_loc, scale, alpha);
        for elem in surfaces {
            let rounded = crate::render::RoundedElement::new(elem, rounded_prog.clone(), radius);
            let rescaled = RescaleRenderElement::from_element(rounded, center, scale_anim);
            elements.push(OutputRenderElements::Window(
                RelocateRenderElement::from_element(rescaled, move_off, Relocate::Relative),
            ));
        }
    }

    // Lower layers (Bottom/Background) → below windows and borders.
    elements.extend(lower_layer_elems.into_iter().map(OutputRenderElements::Layer));

    // Wallpaper — the very back of the stack.
    if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        (0.0, 0.0),
        &surface.wallpaper,
        None,
        None,
        None,
        Kind::Unspecified,
    ) {
        elements.push(OutputRenderElements::Memory(elem));
    }

    // Keep the loop hot while animations are in flight.
    if anims_active {
        state.pending_redraw = true;
    }

    // Theme background (visible only where the wallpaper doesn't cover).
    let clear = Color32F::new(
        theme.background[0], theme.background[1], theme.background[2], 1.0,
    );
    surface.compositor.render_frame(renderer, &elements, clear, FrameFlags::DEFAULT)
        .map_err(|e| anyhow::anyhow!("render_frame: {e:?}"))?;
    surface.compositor.queue_frame(())
        .map_err(|e| anyhow::anyhow!("queue_frame: {e:?}"))?;

    // Frame callbacks — clients only redraw if we tell them this frame shipped.
    // Without these, alacritty draws once and goes silent.
    let time_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u32)
        .unwrap_or(0);
    for window in state.space.elements() {
        if let Some(surf) = window.wl_surface() {
            send_frames_surface_tree(&surf, time_ms);
        }
    }
    // Layer surfaces need frame callbacks too — GTK (vendibar) draws once
    // and then waits on the callback before every subsequent frame. Without
    // this the bar freezes: clock stuck, workspaces never update.
    {
        let layer_map = layer_map_for_output(&surface.output);
        for layer in layer_map.layers() {
            send_frames_surface_tree(layer.wl_surface(), time_ms);
        }
    }

    Ok(())
}

fn send_frames_surface_tree(
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    time_ms: u32,
) {
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

// ── event source handlers ─────────────────────────────────────────────────────

fn on_libinput_event(event: InputEvent<LibinputInputBackend>, app: &mut UdevApp) {
    let state = &mut app.state;
    match event {
        InputEvent::DeviceAdded { device }   => tracing::info!(?device, "input device added"),
        InputEvent::DeviceRemoved { device } => tracing::info!(?device, "input device removed"),

        // ── keyboard ─────────────────────────────────────────────────────────
        InputEvent::Keyboard { event } => {
            let Some(keyboard) = state.seat.get_keyboard() else { return };
            let key_state = event.state();
            let action = keyboard.input::<Option<crate::input::Action>, _>(
                state,
                event.key_code(),
                key_state,
                SERIAL_COUNTER.next_serial(),
                InputEventTrait::time_msec(&event),
                |data, mods, handle| {
                    // Try the modifier-translated sym first, then fall back
                    // to the raw (level-0) syms — without the fallback,
                    // chords like super+shift+1 never match because shift
                    // turns the sym into `exclam`.
                    let sym = handle.modified_sym();
                    crate::input::handle(&data.config, sym.raw(), key_state, mods)
                        .or_else(|| handle.raw_syms().iter().find_map(|s| {
                            crate::input::handle(&data.config, s.raw(), key_state, mods)
                        }))
                        .map_or(FilterResult::Forward, |a| FilterResult::Intercept(Some(a)))
                },
            );
            if let Some(Some(act)) = action {
                if state.run_action(act) {
                    tracing::info!("quit action received");
                }
            }
        }

        // ── pointer motion (relative — typical of mice) ──────────────────────
        InputEvent::PointerMotion { event } => {
            let Some(pointer) = state.seat.get_pointer() else { return };
            let delta_x = event.delta_x();
            let delta_y = event.delta_y();
            state.pointer_location += (delta_x, delta_y).into();
            if let Some(output) = state.space.outputs().next().cloned() {
                if let Some(geo) = state.space.output_geometry(&output) {
                    let max_x = (geo.loc.x + geo.size.w) as f64;
                    let max_y = (geo.loc.y + geo.size.h) as f64;
                    state.pointer_location.x = state.pointer_location.x.clamp(geo.loc.x as f64, max_x);
                    state.pointer_location.y = state.pointer_location.y.clamp(geo.loc.y as f64, max_y);
                }
            }
            // Super+drag in progress: route motion into the drag, not the client.
            if state.drag.is_some() {
                state.drag_update();
                return;
            }
            let location = state.pointer_location;
            let under = state.surface_under(location).map(|(s, p)| (s.into(), p));
            pointer.motion(state, under, &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time:   InputEventTrait::time_msec(&event),
            });
            pointer.frame(state);
            state.pending_redraw = true;
        }

        // ── pointer motion (absolute — touchscreens/tablets) ─────────────────
        InputEvent::PointerMotionAbsolute { event } => {
            let Some(pointer) = state.seat.get_pointer() else { return };
            let Some(output) = state.space.outputs().next().cloned() else { return };
            let Some(geo) = state.space.output_geometry(&output) else { return };
            let pos = event.position_transformed(geo.size);
            state.pointer_location = pos + geo.loc.to_f64();
            let location = state.pointer_location;
            let under = state.surface_under(location).map(|(s, p)| (s.into(), p));
            pointer.motion(state, under, &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time:   InputEventTrait::time_msec(&event),
            });
            pointer.frame(state);
            state.pending_redraw = true;
        }

        // ── click ────────────────────────────────────────────────────────────
        InputEvent::PointerButton { event } => {
            let Some(pointer) = state.seat.get_pointer() else { return };
            let bstate = event.state();
            const BTN_LEFT:  u32 = 0x110;
            const BTN_RIGHT: u32 = 0x111;

            // End an in-flight Super+drag on any release.
            if bstate == smithay::backend::input::ButtonState::Released && state.drag.is_some() {
                state.drag = None;
                return;
            }

            if bstate == smithay::backend::input::ButtonState::Pressed {
                // Super+LeftDrag = move floating, Super+RightDrag = resize.
                let logo = state.seat.get_keyboard()
                    .map(|k| k.modifier_state().logo)
                    .unwrap_or(false);
                let code = event.button_code();
                if logo && (code == BTN_LEFT || code == BTN_RIGHT) {
                    let pos = state.pointer_location;
                    let target = state.space.element_under(pos).map(|(w, _)| w.clone());
                    if let Some(window) = target {
                        let rect = state.workspaces.active_ref().floating.iter()
                            .find(|(w, _)| w == &window)
                            .map(|(_, r)| *r);
                        if let Some(start_rect) = rect {
                            state.focus_window_at_cursor();
                            state.drag = Some(crate::state::Drag {
                                window,
                                resize:     code == BTN_RIGHT,
                                start_ptr:  pos,
                                start_rect,
                            });
                            return;   // the client never sees this press
                        }
                    }
                }
                state.focus_window_at_cursor();
            }
            if let Some(button) = event.button_code().into() {
                pointer.button(state, &ButtonEvent {
                    button,
                    state:  bstate,
                    serial: SERIAL_COUNTER.next_serial(),
                    time:   InputEventTrait::time_msec(&event),
                });
            }
            pointer.frame(state);
        }

        // ── scroll ──────────────────────────────────────────────────────────
        InputEvent::PointerAxis { event } => {
            use smithay::backend::input::{Axis, AxisSource};
            let Some(pointer) = state.seat.get_pointer() else { return };
            let mut frame = AxisFrame::new(InputEventTrait::time_msec(&event))
                .source(AxisSource::Wheel);
            if let Some(h) = event.amount(Axis::Horizontal) {
                frame = frame.value(Axis::Horizontal, h);
            }
            if let Some(v) = event.amount(Axis::Vertical) {
                frame = frame.value(Axis::Vertical, v);
            }
            pointer.axis(state, frame);
            pointer.frame(state);
        }

        // ── 3-finger swipe = workspace switch ───────────────────────────────
        InputEvent::GestureSwipeBegin { event } => {
            use smithay::backend::input::GestureBeginEvent;
            state.swipe = Some((event.fingers(), 0.0));
        }
        InputEvent::GestureSwipeUpdate { event } => {
            use smithay::backend::input::GestureSwipeUpdateEvent as _;
            if let Some((_, dx)) = state.swipe.as_mut() {
                *dx += event.delta_x();
            }
        }
        InputEvent::GestureSwipeEnd { event } => {
            use smithay::backend::input::GestureEndEvent;
            let Some((fingers, dx)) = state.swipe.take() else { return };
            if event.cancelled() || fingers != 3 || dx.abs() < 120.0 { return; }
            // Swiping left moves the viewport right → next workspace.
            let forward = dx < 0.0;
            if let Some(id) = state.workspaces.adjacent_id(forward) {
                state.switch_workspace(id);
            }
        }

        _ => {}
    }
}

fn on_udev_event(event: UdevEvent, _app: &mut UdevApp) {
    match event {
        UdevEvent::Added   { device_id, path } => tracing::info!(?device_id, ?path, "udev: device added"),
        UdevEvent::Changed { device_id }       => tracing::info!(?device_id,        "udev: device changed"),
        UdevEvent::Removed { device_id }       => tracing::info!(?device_id,        "udev: device removed"),
    }
}
