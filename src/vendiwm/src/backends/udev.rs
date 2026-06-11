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
                AsRenderElements, Kind, RenderElement,
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
    // Wallpaper, rescalable so workspace switches can zoom-settle it.
    Wallpaper=RescaleRenderElement<MemoryRenderBufferRenderElement<GlesRenderer>>,
    // Two rescale layers: inner = layout morph (non-uniform, anchored at the
    // window's top-left), outer = open/drag scale (uniform, anchored center).
    Window=RelocateRenderElement<RescaleRenderElement<RescaleRenderElement<crate::render::RoundedElement>>>,
    Pixel=RescaleRenderElement<PixelShaderElement>,
    // Close-animation ghosts — static textures of windows that just died.
    Texture=smithay::backend::renderer::element::texture::TextureRenderElement<smithay::backend::renderer::gles::GlesTexture>,
    // Frosted-glass patches behind overlay surfaces (vendi-menu).
    Blur=crate::render::BlurElement,
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
    let first_crtcs = match initial_surface_setup(&mut app, primary_gpu_node) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(?e, "no usable connector at startup; running headless");
            Vec::new()
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

    // 10. Kick off the first frames so VBlank-driven rendering can begin.
    for crtc in first_crtcs {
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
    let session_lock_state   = smithay::wayland::session_lock::SessionLockManagerState::new::<State, _>(dh, |_| true);
    let primary_selection_state = smithay::wayland::selection::primary_selection::PrimarySelectionState::new::<State>(dh);
    let xdg_decoration_state = smithay::wayland::shell::xdg::decoration::XdgDecorationState::new::<State>(dh);
    let viewporter_state     = smithay::wayland::viewporter::ViewporterState::new::<State>(dh);
    let dmabuf_state         = DmabufState::new();

    let mut seat_state = SeatState::new();
    let mut seat = seat_state.new_wl_seat(dh, "vendi-seat-0");
    let _ = seat.add_keyboard(Default::default(), 200, 25)
        .context("add keyboard to seat")?;
    let _ = seat.add_pointer();

    let config = crate::config::Config::load().unwrap_or_else(|e| {
        tracing::warn!(?e, "config load failed; using empty keybinds");
        crate::config::Config { keybinds: Default::default(), keybinds_pretty: Default::default(), theme: Default::default() }
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
        session_lock_state,
        primary_selection_state,
        xdg_decoration_state,
        viewporter_state,
        seat,
        lock_pending:           None,
        locked:                 false,
        lock_surface:           None,
        space:                  Space::default(),
        popups:                 PopupManager::default(),
        workspaces:             crate::workspaces::Workspaces::new(),
        window_titles:          Default::default(),
        rule_checked:           Default::default(),
        drag:                   None,
        drag_release:           None,
        swipe:                  None,
        overview:               false,
        overview_t:             std::time::Instant::now(),
        screenshot:             None,
        wallpaper_gen:          0,
        vlock: false,
        vlock_input: String::new(),
        vlock_fail: None,
        last_zone:              None,
        open_anims:             Vec::new(),
        ws_anim:                None,
        geo_anims:              Vec::new(),
        closing:                Vec::new(),
        last_geos: HashMap::new(),
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
    /// Separable gaussian blur pass (frosted glass behind vendi-menu).
    pub blur_prog:    GlesTexProgram,
    /// Ping-pong offscreen targets for the blur, at 1/4 output size.
    /// Recreated whenever the output size changes.
    pub blur_texs:    Option<(
        smithay::backend::renderer::gles::GlesTexture,
        smithay::backend::renderer::gles::GlesTexture,
    )>,
    /// Snapshot of every mapped window for the close ghost, keyed by window
    /// id: (previous copy, current copy, time of current copy). Owned blits,
    /// refreshed every ~300ms. Two generations because clients commit junk
    /// on their way out — Firefox's final buffer is fully transparent — so
    /// the ghost prefers the previous, pre-teardown snapshot.
    pub tex_stash:    HashMap<u32, (
        Option<smithay::backend::renderer::gles::GlesTexture>,
        smithay::backend::renderer::gles::GlesTexture,
        std::time::Instant,
    )>,
    /// Per-window focus-ring blend (0 = inactive color, 1 = accent), eased
    /// toward its target by wall-clock time (renders aren't evenly spaced).
    pub focus_anim:   HashMap<u32, f32>,
    /// When the previous frame was rendered — the dt for the easing above.
    pub last_tick:    std::time::Instant,
    /// In-flight close animations: stable element id, ghost texture, the
    /// window's final rect, start time.
    pub closing_anims: Vec<(
        smithay::backend::renderer::element::Id,
        smithay::backend::renderer::gles::GlesTexture,
        smithay::utils::Rectangle<i32, smithay::utils::Logical>,
        std::time::Instant,
    )>,
}

pub struct SurfaceState {
    pub output:     Output,
    pub compositor: GbmDrmCompositor,
    /// Output-sized wallpaper (user image or the built-in gradient).
    pub wallpaper:  MemoryRenderBuffer,
    /// Matches state.wallpaper_gen when `wallpaper` is current.
    pub wallpaper_gen: u64,
    /// Which connector drives this surface (hotplug bookkeeping).
    pub connector:  connector::Handle,
    /// The mode we set — a different preferred mode on rescan means the
    /// monitor changed resolution and the surface must be rebuilt.
    pub mode:       smithay::reexports::drm::control::Mode,
    /// The wl_output global, removed when the connector goes away.
    pub global:     smithay::reexports::wayland_server::backend::GlobalId,
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
        let blur_prog = renderer.compile_custom_texture_shader(
            crate::render::BLUR_FRAG,
            &[UniformName::new("dir", UniformType::_2f)],
        ).map_err(|e| anyhow::anyhow!("compile blur shader: {e:?}"))?;

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
            blur_prog,
            blur_texs: None,
            tex_stash: HashMap::new(),
            focus_anim: HashMap::new(),
            last_tick: std::time::Instant::now(),
            closing_anims: Vec::new(),
        };
        Ok((dev, notifier))
    }
}

// ── surface setup ─────────────────────────────────────────────────────────────

/// Preferred mode if flagged, else the first listed one.
fn pick_mode(connector: &connector::Info) -> Option<smithay::reexports::drm::control::Mode> {
    let idx = connector.modes().iter()
        .position(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
        .unwrap_or(0);
    connector.modes().get(idx).copied()
}

/// Bring up every connected connector at startup. Returns the CRTCs so the
/// caller can kick their first frames.
fn initial_surface_setup(app: &mut UdevApp, node: DrmNode) -> Result<Vec<crtc::Handle>> {
    let connected: Vec<connector::Info> = app.udev.drm_devices.get(&node)
        .ok_or_else(|| anyhow::anyhow!("device not found: {node:?}"))?
        .connectors.iter()
        .filter(|c| c.state() == connector::State::Connected)
        .cloned()
        .collect();
    if connected.is_empty() {
        tracing::warn!("no connected connector");
        return Ok(Vec::new());
    }
    let mut crtcs = Vec::new();
    for info in connected {
        match connect_connector(app, node, &info) {
            Ok(crtc) => crtcs.push(crtc),
            Err(e) => tracing::warn!(?e, connector = ?info.interface(), "connector bringup failed"),
        }
    }
    // Start the pointer at the centre of the first screen so the cursor is
    // visible before the user has nudged the mouse.
    if let Some(geo) = app.state.space.outputs().next()
        .and_then(|o| app.state.space.output_geometry(o))
    {
        app.state.pointer_location =
            (geo.loc.x as f64 + geo.size.w as f64 / 2.0,
             geo.loc.y as f64 + geo.size.h as f64 / 2.0).into();
    }
    app.state.relayout();
    Ok(crtcs)
}

/// Bring up one connector: pick a mode, find a free CRTC, create a
/// DrmSurface + DrmCompositor + a smithay Output, and map the Output into
/// the Space to the right of everything already there.
fn connect_connector(
    app: &mut UdevApp,
    node: DrmNode,
    connector: &connector::Info,
) -> Result<crtc::Handle> {
    let UdevApp { state, udev, display_handle } = app;
    let device = udev.drm_devices.get_mut(&node)
        .ok_or_else(|| anyhow::anyhow!("device not found: {node:?}"))?;

    let drm_mode = pick_mode(connector)
        .ok_or_else(|| anyhow::anyhow!("connector has no modes"))?;

    // Find a CRTC reachable by one of this connector's encoders that isn't
    // already driving another surface.
    let resources = device.drm.resource_handles()
        .map_err(|e| anyhow::anyhow!("resource_handles: {e:?}"))?;
    let mut chosen_crtc: Option<crtc::Handle> = None;
    'outer: for enc_handle in connector.encoders() {
        let Ok(enc) = device.drm.get_encoder(*enc_handle) else { continue };
        for crtc in resources.filter_crtcs(enc.possible_crtcs()) {
            if device.surfaces.contains_key(&crtc) { continue; }
            chosen_crtc = Some(crtc);
            break 'outer;
        }
    }
    let crtc = chosen_crtc.ok_or_else(|| anyhow::anyhow!("no usable CRTC for connector"))?;

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
    // New outputs land to the right of everything already mapped.
    let next_x = state.space.outputs()
        .filter_map(|o| state.space.output_geometry(o))
        .map(|g| g.loc.x + g.size.w)
        .max()
        .unwrap_or(0);
    let wl_mode = WlMode::from(drm_mode);
    output.change_current_state(
        Some(wl_mode),
        Some(Transform::Normal),
        Some(Scale::Integer(1)),
        Some((next_x, 0).into()),
    );
    output.set_preferred(wl_mode);
    let global = output.create_global::<State>(display_handle);
    state.space.map_output(&output, (next_x, 0));

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

    let mode_size = drm_mode.size();
    tracing::info!(
        crtc = ?crtc,
        connector = %output_name,
        mode = ?(mode_size, drm_mode.vrefresh()),
        at = next_x,
        "DRM output up",
    );

    let wallpaper = crate::render::wallpaper_buffer(
        mode_size.0 as i32,
        mode_size.1 as i32,
        state.config.theme.wallpaper.as_deref(),
        state.config.theme.background,
        state.config.theme.accent,
    );

    device.surfaces.insert(crtc, SurfaceState {
        output,
        compositor,
        wallpaper,
        wallpaper_gen: state.wallpaper_gen,
        connector: connector.handle(),
        mode: drm_mode,
        global,
    });
    Ok(crtc)
}

/// React to a udev "changed" event: re-probe connectors, tear down surfaces
/// whose monitor vanished or switched resolution, bring up new ones, then
/// re-pack the remaining outputs left-to-right and keep the pointer inside.
fn rescan_connectors(app: &mut UdevApp, node: DrmNode) {
    // 1. Re-probe.
    let (stale, fresh): (Vec<crtc::Handle>, Vec<connector::Info>) = {
        let Some(device) = app.udev.drm_devices.get_mut(&node) else { return };
        let Ok(resources) = device.drm.resource_handles() else { return };
        let mut connectors = Vec::new();
        for c in resources.connectors() {
            // No force-probe: the kernel re-probed before emitting the udev
            // change event, and a forced probe would override manual sysfs
            // status (used by tests and `video=` overrides).
            if let Ok(info) = device.drm.get_connector(*c, false) {
                connectors.push(info);
            }
        }
        device.connectors = connectors.clone();

        // 2. Surfaces to drop: connector gone, unplugged, or new mode.
        let stale = device.surfaces.iter()
            .filter_map(|(crtc, s)| {
                let info = connectors.iter().find(|c| c.handle() == s.connector);
                let connected = matches!(info.map(|c| c.state()), Some(connector::State::Connected));
                let same_mode = info.and_then(pick_mode) == Some(s.mode);
                (!connected || !same_mode).then_some(*crtc)
            })
            .collect();

        // 3. Connected connectors that have no surface yet.
        let have: Vec<connector::Handle> = device.surfaces.values().map(|s| s.connector).collect();
        let fresh = connectors.into_iter()
            .filter(|c| c.state() == connector::State::Connected && !have.contains(&c.handle()))
            .collect();
        (stale, fresh)
    };

    for crtc in stale {
        let Some(device) = app.udev.drm_devices.get_mut(&node) else { return };
        if let Some(s) = device.surfaces.remove(&crtc) {
            tracing::info!(output = %s.output.name(), "DRM output down");
            app.state.space.unmap_output(&s.output);
            app.display_handle.remove_global::<State>(s.global);
        }
    }

    let mut new_crtcs = Vec::new();
    for info in fresh {
        match connect_connector(app, node, &info) {
            Ok(crtc) => new_crtcs.push(crtc),
            Err(e) => tracing::warn!(?e, "connector bringup failed"),
        }
    }

    // 4. Re-pack outputs left-to-right in stable connector order so removals
    //    don't leave gaps the pointer could fall into.
    {
        let Some(device) = app.udev.drm_devices.get_mut(&node) else { return };
        let mut surfaces: Vec<&SurfaceState> = device.surfaces.values().collect();
        surfaces.sort_by_key(|s| s.output.name());
        let mut x = 0;
        for s in surfaces {
            let size = s.output.current_mode().map(|m| m.size).unwrap_or_default();
            s.output.change_current_state(None, None, None, Some((x, 0).into()));
            app.state.space.map_output(&s.output, (x, 0));
            x += size.w;
        }
    }

    clamp_pointer(&mut app.state);
    app.state.relayout();
    app.state.pending_redraw = true;

    // Kick the first frame of any new surface so its VBlank loop starts.
    for crtc in new_crtcs {
        if let Err(e) = render_surface(app, node, crtc) {
            tracing::warn!(?e, "post-hotplug render_surface");
        }
    }
}

/// Keep the pointer inside the union of output rectangles (snap to the
/// nearest point of the nearest output when it ends up in a dead zone).
fn clamp_pointer(state: &mut State) {
    let rects: Vec<smithay::utils::Rectangle<i32, smithay::utils::Logical>> = state.space.outputs()
        .filter_map(|o| state.space.output_geometry(o))
        .collect();
    if rects.is_empty() { return; }
    let p = state.pointer_location;
    if rects.iter().any(|r| r.to_f64().contains(p)) { return; }
    let mut best = (f64::MAX, p);
    for r in rects {
        let cx = p.x.clamp(r.loc.x as f64, (r.loc.x + r.size.w) as f64 - 1.0);
        let cy = p.y.clamp(r.loc.y as f64, (r.loc.y + r.size.h) as f64 - 1.0);
        let d = (cx - p.x).powi(2) + (cy - p.y).powi(2);
        if d < best.0 { best = (d, (cx, cy).into()); }
    }
    state.pointer_location = best.1;
}

/// Render one frame for `crtc` on `node`. Gathers elements from the space,
/// asks the DrmCompositor to compose them, and queues the frame for the next
/// VBlank.
/// Blit a (possibly client-owned) texture into a fresh one the compositor
/// owns — used to keep close-ghost pixels alive past the client's buffer.
fn copy_texture(
    renderer: &mut GlesRenderer,
    src: &smithay::backend::renderer::gles::GlesTexture,
) -> Option<smithay::backend::renderer::gles::GlesTexture> {
    use smithay::backend::renderer::{Bind, Frame, Offscreen, Renderer as _, Texture};
    let size = src.size();
    if size.w <= 0 || size.h <= 0 { return None; }
    let mut dst: smithay::backend::renderer::gles::GlesTexture = renderer
        .create_buffer(smithay::backend::allocator::Fourcc::Abgr8888, size)
        .map_err(|e| tracing::warn!(?e, "copy_texture: create_buffer")).ok()?;
    {
        let phys = smithay::utils::Size::<i32, smithay::utils::Physical>::from((size.w, size.h));
        let full = smithay::utils::Rectangle::from_size(phys);
        let mut fb = renderer.bind(&mut dst)
            .map_err(|e| tracing::warn!(?e, "copy_texture: bind")).ok()?;
        let mut frame = renderer.render(&mut fb, phys, Transform::Normal)
            .map_err(|e| tracing::warn!(?e, "copy_texture: render")).ok()?;
        frame.render_texture_from_to(
            src,
            smithay::utils::Rectangle::from_size(size).to_f64(),
            full,
            &[full],
            &[],
            Transform::Normal,
            1.0,
            None,
            &[],
        ).map_err(|e| tracing::warn!(?e, "copy_texture: blit")).ok()?;
        // Block until the blit lands — the source may die right after.
        frame.finish()
            .map_err(|e| tracing::warn!(?e, "copy_texture: finish")).ok()?
            .wait().ok()?;
    }
    Some(dst)
}

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
    let blur_prog    = device.blur_prog.clone();
    let blur_texs     = &mut device.blur_texs;
    let tex_stash     = &mut device.tex_stash;
    let focus_anim    = &mut device.focus_anim;
    let last_tick     = &mut device.last_tick;
    let closing_anims = &mut device.closing_anims;
    let surface  = device.surfaces.get_mut(&crtc)
        .ok_or_else(|| anyhow::anyhow!("surface not found"))?;

    // Wallpaper changed over IPC: rebuild this output's buffer once.
    if surface.wallpaper_gen != state.wallpaper_gen {
        let mode_size = surface.mode.size();
        surface.wallpaper = crate::render::wallpaper_buffer(
            mode_size.0 as i32,
            mode_size.1 as i32,
            state.config.theme.wallpaper.as_deref(),
            state.config.theme.background,
            state.config.theme.accent,
        );
        surface.wallpaper_gen = state.wallpaper_gen;
    }

    // This output's place in the global layout: every element position below
    // is global-logical and must be shifted into output-local space.
    let out_loc = state.space.output_geometry(&surface.output)
        .map(|g| g.loc)
        .unwrap_or_default();

    // vendi-lock: while locked nothing of the desktop may reach the frame —
    // windows, layers, ghosts, and the cursor are all skipped below.
    let locked = state.vlock;

    let scale = smithay::utils::Scale::from(1.0_f64);

    // ── session lock: render ONLY the lock surface ──────────────────────────
    // From the moment a lock is requested, nothing of the desktop may reach
    // the screen — black until the client (swaylock) maps its surface. The
    // lock is confirmed to the client only after the first locked frame has
    // been queued, per the ext-session-lock spec.
    if state.is_locked() {
        let mut elements: Vec<FrameElement> = Vec::new();
        let pointer_phys = smithay::utils::Point::<f64, smithay::utils::Physical>::from((
            state.pointer_location.x - cursor.hotspot.0 as f64,
            state.pointer_location.y - cursor.hotspot.1 as f64,
        ));
        if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
            renderer, pointer_phys, &cursor.buffer, None, None, None, Kind::Cursor,
        ) {
            elements.push(OutputRenderElements::Memory(elem));
        }
        if let Some(lock) = &state.lock_surface {
            elements.extend(
                smithay::backend::renderer::element::surface::render_elements_from_surface_tree::<
                    _, WaylandSurfaceRenderElement<GlesRenderer>,
                >(renderer, lock.wl_surface(), (0, 0), scale, 1.0, Kind::Unspecified)
                .into_iter()
                .map(OutputRenderElements::Layer),
            );
        }
        let res = surface.compositor.render_frame(renderer, &elements, Color32F::new(0.0, 0.0, 0.0, 1.0), FrameFlags::DEFAULT)
            .map_err(|e| anyhow::anyhow!("render_frame: {e:?}"))?;
        if !res.is_empty {
            surface.compositor.queue_frame(())
                .map_err(|e| anyhow::anyhow!("queue_frame: {e:?}"))?;
        }
        if let Some(locker) = state.lock_pending.take() {
            locker.lock();
            state.locked = true;
            tracing::info!("session locked");
        }
        let time_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u32)
            .unwrap_or(0);
        if let Some(lock) = &state.lock_surface {
            send_frames_surface_tree(lock.wl_surface(), time_ms);
        }
        return Ok(());
    }

    // ── animation clocks ────────────────────────────────────────────────────
    // Open: fade + scale-in per window. Workspace switch: the whole desk
    // fades + settles. Eased with cubic ease-out; while anything is in
    // flight we keep pending_redraw set so the loop renders every tick.
    const OPEN_MS:  f32 = 260.0;
    const WS_MS:    f32 = 300.0;
    const MORPH_MS: f32 = 230.0;
    const DRAG_MS:  f32 = 120.0;
    fn ease_out(t: f32) -> f32 { 1.0 - (1.0 - t).powi(3) }
    // Spring-ish: overshoots the target by ~10% then settles — the iOS feel.
    fn ease_out_back(t: f32) -> f32 {
        const C1: f32 = 1.70158;
        const C3: f32 = C1 + 1.0;
        1.0 + C3 * (t - 1.0).powi(3) + C1 * (t - 1.0).powi(2)
    }
    let now = std::time::Instant::now();
    // Wall-clock step for exponential fades: renders are not evenly spaced
    // (the tick path can run far faster than vblank), so per-frame constants
    // would make fade speed depend on load.
    let dt = now.duration_since(*last_tick).as_secs_f32().min(0.1);
    *last_tick = now;
    // ~63% of the remaining distance per 70ms; visually settled in ~250ms.
    let fade_k = 1.0 - (-dt / 0.070).exp();
    state.open_anims.retain(|(w, t)| {
        smithay::utils::IsAlive::alive(w)
            && t.map(|t| (now.duration_since(t).as_secs_f32() * 1000.0) < OPEN_MS)
                .unwrap_or(true)
    });
    state.geo_anims.retain(|(w, _, t)| {
        smithay::utils::IsAlive::alive(w)
            && (now.duration_since(*t).as_secs_f32() * 1000.0) < MORPH_MS
    });
    let ws_progress = state.ws_anim.map(|(t, _)| now.duration_since(t).as_secs_f32() * 1000.0 / WS_MS);
    let ws_dir = state.ws_anim.map(|(_, d)| d).unwrap_or(0);
    if ws_progress.map(|p| p >= 1.0).unwrap_or(false) {
        state.ws_anim = None;
    }
    // The incoming desk fades in and slides from the side it lives on.
    let (ws_alpha, ws_scale, ws_off) = match ws_progress.filter(|p| *p < 1.0) {
        Some(p) => {
            let e = ease_out(p);
            (0.25 + 0.75 * e, 0.97 + 0.03 * e as f64, (ws_dir as f32 * 46.0 * (1.0 - e)).round() as i32)
        }
        None => (1.0, 1.0, 0),
    };
    // Close animations: pair windows that died since last frame with their
    // stashed textures; the ghosts fade + shrink in place.
    const CLOSE_MS: f32 = 200.0;
    closing_anims.retain(|(_, _, _, t)| {
        (now.duration_since(*t).as_secs_f32() * 1000.0) < CLOSE_MS
    });
    for (id, geo) in state.closing.drain(..) {
        if let Some((prev, cur, _)) = tex_stash.remove(&id) {
            // Clients repaint blank/transparent on their way out; the
            // previous snapshot is the window as the user last saw it.
            let tex = prev.unwrap_or(cur);
            closing_anims.push((
                smithay::backend::renderer::element::Id::new(),
                tex,
                geo,
                now,
            ));
        }
    }

    // Overview: windows render at their grid cells instead of their real
    // geometry; the wallpaper dims underneath. The dim eases over the same
    // span as the geo morphs so enter/exit feel like one motion.
    const OVERVIEW_MS: f32 = 220.0;
    // The exit animation still needs the layout (hidden thumbnails fade out
    // at their cells), so keep it around for one morph span after closing.
    let ov_exit = !state.overview
        && (now.duration_since(state.overview_t).as_secs_f32() * 1000.0) < OVERVIEW_MS;
    let ov_layout = if state.overview || ov_exit {
        Some(state.overview_layout())
    } else {
        None
    };
    let overview_cells: Vec<(smithay::desktop::Window, smithay::utils::Rectangle<i32, smithay::utils::Logical>)> =
        if state.overview {
            ov_layout.as_ref().map(|l| {
                l.cells.iter().map(|(w, r, _)| (w.clone(), *r)).collect()
            }).unwrap_or_default()
        } else {
            Vec::new()
        };
    let ov_e = ease_out(
        (now.duration_since(state.overview_t).as_secs_f32() * 1000.0 / OVERVIEW_MS).min(1.0),
    );
    let mut wallpaper_alpha = if locked { 0.30 } else if state.overview { 1.0 - 0.55 * ov_e } else { 0.45 + 0.55 * ov_e };
    // Workspace switches dim the wallpaper through the transition so even a
    // switch between empty desks reads as motion.
    if let Some(p) = ws_progress.filter(|p| *p < 1.0) {
        let e = ease_out(p);
        wallpaper_alpha *= 0.72 + 0.28 * (2.0 * (e - 0.5)).abs();
    }

    if state.drag_release.as_ref()
        .map(|(_, t)| (now.duration_since(*t).as_secs_f32() * 1000.0) >= DRAG_MS)
        .unwrap_or(false)
    {
        state.drag_release = None;
    }
    let anims_active = state.ws_anim.is_some()
        || state.drag_release.is_some()
        || state.vlock_fail.map(|t| now.duration_since(t).as_secs_f32() < 1.0).unwrap_or(false)
        || !state.open_anims.is_empty()
        || !state.geo_anims.is_empty()
        || !closing_anims.is_empty()
        || state.drag.is_some()
        || (now.duration_since(state.overview_t).as_secs_f32() * 1000.0) < OVERVIEW_MS;
    let theme = state.config.theme.clone();
    let mut upper_layer_elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = Vec::new();
    let mut lower_layer_elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = Vec::new();
    // Logical rects that want a blurred-desktop slab behind them.
    let mut blur_rects: Vec<smithay::utils::Rectangle<i32, smithay::utils::Logical>> = Vec::new();
    // A fullscreen window hides the Top layer (the bar) — only Overlay
    // surfaces (e.g. a lock screen) stay above it, per the wlr spec.
    let fullscreen_active = state.workspaces.active_ref().fullscreen.is_some();
    if !locked {
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
            // The menu gets a frosted-glass slab of the desktop behind it.
            if theme.blur && layer.namespace() == "vendi-menu" {
                blur_rects.push(geo);
            }
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
        state.pointer_location.x - out_loc.x as f64 - cursor.hotspot.0 as f64,
        state.pointer_location.y - out_loc.y as f64 - cursor.hotspot.1 as f64,
    ));
    if !locked {
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
    }
    // Screenshots skip everything up to here (i.e. the cursor).
    let after_cursor = elements.len();

    // Upper layers (Top/Overlay) → above windows but below the cursor.
    elements.extend(upper_layer_elems.into_iter().map(OutputRenderElements::Layer));
    // Everything pushed from here down is "the desktop" — the blur pass at
    // the bottom of this function re-renders elements[blur_mark..] into an
    // offscreen target, and the frosted patches are inserted at this index
    // (directly beneath the menu, above all desktop content).
    let blur_mark = elements.len();

    // Close ghosts — above live windows (the dying window was usually on top).
    let ctx = smithay::backend::renderer::Renderer::context_id(renderer);
    for (eid, tex, geo, t) in closing_anims.iter().filter(|_| !locked) {
        let e = ease_out((now.duration_since(*t).as_secs_f32() * 1000.0 / CLOSE_MS).min(1.0));
        let shrink = 1.0 - 0.15 * e as f64;
        let size = smithay::utils::Size::<i32, smithay::utils::Logical>::from((
            ((geo.size.w as f64 * shrink) as i32).max(1),
            ((geo.size.h as f64 * shrink) as i32).max(1),
        ));
        let loc = smithay::utils::Point::<f64, smithay::utils::Physical>::from((
            (geo.loc.x - out_loc.x) as f64 + (geo.size.w - size.w) as f64 / 2.0,
            (geo.loc.y - out_loc.y) as f64 + (geo.size.h - size.h) as f64 / 2.0,
        ));
        let ghost = smithay::backend::renderer::element::texture::TextureRenderElement::from_static_texture(
            eid.clone(),
            ctx.clone(),
            loc,
            tex.clone(),
            1,
            Transform::Normal,
            Some(1.0 - e),
            None,
            Some(size),
            None,
            Kind::Unspecified,
        );
        elements.push(OutputRenderElements::Texture(ghost));
    }

    // Windows + borders, topmost first. Each window's surfaces go through
    // the rounded-corner shader; its border is an SDF ring drawn just above
    // its own edge; both share the window's animation transform (fade +
    // scale around the center, glide between tile slots).
    let border_w = theme.border;
    let focused_surf = state.seat.get_keyboard().and_then(|k| k.current_focus());
    let fullscreen = state.workspaces.active_ref().fullscreen.clone();
    let stacked: Vec<_> = if locked { Vec::new() } else { state.space.elements().cloned().collect() };
    let mut live_ids: Vec<u32> = Vec::with_capacity(stacked.len());
    for window in stacked.iter().rev() {
        let Some(geo) = state.space.element_geometry(window) else { continue };

        // Stash this frame's texture so a close next frame can ghost it.
        if let Some(surf) = window.wl_surface() {
            let wid = crate::state::window_id(window);
            live_ids.push(wid);
            state.last_geos.insert(wid, geo);
            let tex = smithay::backend::renderer::utils::with_renderer_surface_state(
                &surf,
                |s| s.texture(ctx.clone()).cloned(),
            ).flatten();
            if let Some(tex) = tex {
                let due = !matches!(
                    tex_stash.get(&wid),
                    Some((_, _, t)) if now.duration_since(*t).as_millis() < 300
                );
                if due {
                    if let Some(copy) = copy_texture(renderer, &tex) {
                        let prev = tex_stash.remove(&wid).map(|(_, cur, _)| cur);
                        tex_stash.insert(wid, (prev, copy, now));
                    }
                }
            }
        }

        // The open clock starts on the first frame the window has committed
        // content — starting it at new_toplevel let the configure round-trip
        // eat most of the animation, so windows popped in half-faded. Until
        // then the window isn't drawn at all.
        let committed = window.geometry().size;
        if committed.w > 0 && committed.h > 0 {
            if let Some((_, started)) = state.open_anims.iter_mut().find(|(w, _)| w == window) {
                if started.is_none() { *started = Some(now); }
            }
        } else if state.open_anims.iter().any(|(w, _)| w == window) {
            continue;
        }

        // Per-window open animation on top of the workspace-switch one.
        // Alpha eases out plainly; scale takes the spring (slight overshoot).
        let open_t = state.open_anims.iter()
            .find(|(w, _)| w == window)
            .and_then(|(_, t)| *t)
            .map(|t| (now.duration_since(t).as_secs_f32() * 1000.0 / OPEN_MS).min(1.0));
        let alpha = ws_alpha * open_t.map(ease_out).unwrap_or(1.0);
        // Super+drag pick-up: ease in a slight grow while the grab holds,
        // and ease it back out after release (put-down).
        let drag_scale: f64 = state.drag.as_ref()
            .filter(|d| &d.window == window && !d.resize)
            .map(|d| {
                let e = ease_out((now.duration_since(d.started).as_secs_f32() * 1000.0 / DRAG_MS).min(1.0));
                1.0 + 0.02 * e as f64
            })
            .or_else(|| state.drag_release.as_ref()
                .filter(|(w, _)| w == window)
                .map(|(_, t)| {
                    let e = ease_out((now.duration_since(*t).as_secs_f32() * 1000.0 / DRAG_MS).min(1.0));
                    1.0 + 0.02 * (1.0 - e as f64)
                }))
            .unwrap_or(1.0);
        let scale_anim: f64 = ws_scale
            * open_t.map(|t| 0.90 + 0.10 * ease_out_back(t) as f64).unwrap_or(1.0)
            * drag_scale;

        // Layout morph: interpolate the whole rect (location AND size) from
        // the old slot, so moves, resizes, and fullscreen toggles glide.
        // The workspace slide rides on the same rect. In overview the
        // destination is the window's grid cell, not its real geometry.
        let dest = overview_cells.iter()
            .find(|(w, _)| w == window)
            .map(|(_, r)| *r)
            .unwrap_or(geo);
        let target = state.geo_anims.iter()
            .find(|(w, _, _)| w == window)
            .map(|(_, old, t)| {
                let e = ease_out((now.duration_since(*t).as_secs_f32() * 1000.0 / MORPH_MS).min(1.0));
                let l = |a: i32, b: i32| (a as f32 + (b - a) as f32 * e).round() as i32;
                smithay::utils::Rectangle::<i32, smithay::utils::Logical>::new(
                    (l(old.loc.x, dest.loc.x) + ws_off, l(old.loc.y, dest.loc.y)).into(),
                    (l(old.size.w, dest.size.w).max(1), l(old.size.h, dest.size.h).max(1)).into(),
                )
            })
            .unwrap_or_else(|| smithay::utils::Rectangle::new(
                (dest.loc.x + ws_off, dest.loc.y).into(),
                dest.size,
            ));

        // Shift the rect pair into this output's local space (multi-monitor:
        // a window at global x=2000 is at x=80 on a second 1920-wide output).
        let target = { let mut t = target; t.loc -= out_loc; t };
        let geo = { let mut g = geo; g.loc -= out_loc; g };

        let is_fullscreen = fullscreen.as_ref() == Some(window);
        let radius = if is_fullscreen { 0.0 } else { theme.radius };

        // Border ring, drawn around the interpolated rect (skip on fullscreen).
        if !is_fullscreen {
            let win_surf = window.wl_surface();
            let focused = matches!((&focused_surf, &win_surf), (Some(f), Some(s)) if **s == *f);
            // Fade the ring between inactive and accent instead of snapping.
            let c = {
                let wid = crate::state::window_id(window);
                let target: f32 = if focused { 1.0 } else { 0.0 };
                let f = focus_anim.entry(wid).or_insert(target);
                *f += (target - *f) * fade_k;
                if (*f - target).abs() > 0.01 {
                    state.pending_redraw = true;
                } else {
                    *f = target;
                }
                let t = *f;
                [
                    theme.inactive[0] + (theme.accent[0] - theme.inactive[0]) * t,
                    theme.inactive[1] + (theme.accent[1] - theme.inactive[1]) * t,
                    theme.inactive[2] + (theme.accent[2] - theme.inactive[2]) * t,
                    theme.inactive[3] + (theme.accent[3] - theme.inactive[3]) * t,
                ]
            };
            let area = smithay::utils::Rectangle::<i32, smithay::utils::Logical>::new(
                (target.loc.x - border_w, target.loc.y - border_w).into(),
                (target.size.w + border_w * 2, target.size.h + border_w * 2).into(),
            );
            let ring_center = smithay::utils::Point::<i32, smithay::utils::Physical>::from((
                target.loc.x + target.size.w / 2,
                target.loc.y + target.size.h / 2,
            ));
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
                RescaleRenderElement::from_element(ring, ring_center, scale_anim),
            ));
        }

        // Window content (toplevel + subsurfaces + popups), rounded. Inner
        // rescale = morph from the committed size to the interpolated one
        // (anchored at the content's top-left); outer rescale = open/drag
        // scale (anchored at the on-screen center, pre-relocate coords);
        // relocate shifts everything to the interpolated location.
        let render_loc = (geo.loc - window.geometry().loc).to_physical_precise_round(scale);
        let surfaces: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
            window.render_elements(renderer, render_loc, scale, alpha);
        let morph_scale = smithay::utils::Scale {
            x: target.size.w as f64 / geo.size.w.max(1) as f64,
            y: target.size.h as f64 / geo.size.h.max(1) as f64,
        };
        let anchor: smithay::utils::Point<i32, smithay::utils::Physical> =
            geo.loc.to_physical_precise_round(scale);
        let content_center = smithay::utils::Point::<i32, smithay::utils::Physical>::from((
            geo.loc.x + target.size.w / 2,
            geo.loc.y + target.size.h / 2,
        ));
        let off = (target.loc.x - geo.loc.x, target.loc.y - geo.loc.y);
        for elem in surfaces {
            let rounded = crate::render::RoundedElement::new(elem, rounded_prog.clone(), radius);
            let morphed = RescaleRenderElement::from_element(rounded, anchor, morph_scale);
            let rescaled = RescaleRenderElement::from_element(morphed, content_center, scale_anim);
            elements.push(OutputRenderElements::Window(
                RelocateRenderElement::from_element(rescaled, off, Relocate::Relative),
            ));
        }
    }

    // Drop stashed textures only when the window is truly gone (their close
    // ghosts, if any, were taken out of the stash above). Unmapped-but-alive
    // windows (hidden workspaces, clients that unmap before destroying —
    // Firefox does) keep theirs so the close ghost still has pixels.
    {
        let alive_ids: Vec<u32> = state.workspaces.all_windows().iter()
            .map(crate::state::window_id)
            .collect();
        tex_stash.retain(|id, _| alive_ids.contains(id) || live_ids.contains(id));
        focus_anim.retain(|id, _| alive_ids.contains(id));
    }

    // Overview extras: thumbnails of windows on hidden workspaces (they're
    // unmapped, so the loop above never sees them) and a ring around every
    // workspace panel. Both fade with the overview.
    if let Some(layout) = &ov_layout {
        let a_ov = if state.overview { ov_e } else { 1.0 - ov_e };
        let active_id = state.workspaces.active_id();
        for (window, cell, ws) in &layout.cells {
            if *ws == active_id { continue; }
            if !smithay::utils::IsAlive::alive(window) { continue; }
            let committed = window.geometry().size;
            if committed.w <= 0 || committed.h <= 0 { continue; }

            let cell = { let mut c = *cell; c.loc -= out_loc; c };
            let render_loc = (cell.loc - window.geometry().loc).to_physical_precise_round(scale);
            let surfaces: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                window.render_elements(renderer, render_loc, scale, a_ov);
            let morph_scale = smithay::utils::Scale {
                x: cell.size.w as f64 / committed.w.max(1) as f64,
                y: cell.size.h as f64 / committed.h.max(1) as f64,
            };
            let anchor: smithay::utils::Point<i32, smithay::utils::Physical> =
                cell.loc.to_physical_precise_round(scale);
            for elem in surfaces {
                let rounded = crate::render::RoundedElement::new(elem, rounded_prog.clone(), theme.radius);
                let morphed = RescaleRenderElement::from_element(rounded, anchor, morph_scale);
                let rescaled = RescaleRenderElement::from_element(morphed, anchor, 1.0);
                elements.push(OutputRenderElements::Window(
                    RelocateRenderElement::from_element(rescaled, (0, 0), Relocate::Relative),
                ));
            }
        }
        for (_, rect, is_active) in &layout.panels {
            let c = if *is_active { theme.accent } else { theme.inactive };
            let area = smithay::utils::Rectangle::<i32, smithay::utils::Logical>::new(
                (rect.loc.x - out_loc.x - 6, rect.loc.y - out_loc.y - 6).into(),
                (rect.size.w + 12, rect.size.h + 12).into(),
            );
            let ring = PixelShaderElement::new(
                border_prog.clone(),
                area,
                None,
                0.85 * a_ov,
                vec![
                    Uniform::new("color", c),
                    Uniform::new("radius", 14.0),
                    Uniform::new("thickness", 2.0),
                ],
                Kind::Unspecified,
            );
            elements.push(OutputRenderElements::Pixel(
                RescaleRenderElement::from_element(ring, (0, 0).into(), 1.0),
            ));
        }
    }

    // Lower layers (Bottom/Background) → below windows and borders.
    if !locked {
        elements.extend(lower_layer_elems.into_iter().map(OutputRenderElements::Layer));
    }

    // vendi-lock UI: a row of accent password dots over the dimmed
    // wallpaper (red while an attempt just failed); a hollow hint ring
    // marks the locked state while the buffer is empty.
    if locked {
        let osize = state.space.output_geometry(&surface.output)
            .map(|g| g.size)
            .unwrap_or((1, 1).into());
        let failed = state.vlock_fail
            .map(|t| now.duration_since(t).as_secs_f32() < 0.8)
            .unwrap_or(false);
        let color = if failed { [0.886, 0.137, 0.102, 1.0] } else { theme.accent };
        let n = state.vlock_input.chars().count().min(32) as i32;
        let cy = (osize.h as f32 * 0.60) as i32;
        if n == 0 {
            let ring = PixelShaderElement::new(
                border_prog.clone(),
                smithay::utils::Rectangle::<i32, smithay::utils::Logical>::new(
                    (osize.w / 2 - 14, cy - 14).into(), (28, 28).into(),
                ),
                None,
                0.9,
                vec![
                    Uniform::new("color", color),
                    Uniform::new("radius", 14.0),
                    Uniform::new("thickness", 2.0),
                ],
                Kind::Unspecified,
            );
            elements.push(OutputRenderElements::Pixel(
                RescaleRenderElement::from_element(ring, (0, 0).into(), 1.0),
            ));
        }
        let gap = 34;
        let x0 = osize.w / 2 - (n * gap) / 2;
        for i in 0..n {
            let dot = PixelShaderElement::new(
                border_prog.clone(),
                smithay::utils::Rectangle::<i32, smithay::utils::Logical>::new(
                    (x0 + i * gap + gap / 2 - 7, cy - 7).into(), (14, 14).into(),
                ),
                None,
                1.0,
                vec![
                    Uniform::new("color", color),
                    Uniform::new("radius", 7.0),
                    Uniform::new("thickness", 7.0),
                ],
                Kind::Unspecified,
            );
            elements.push(OutputRenderElements::Pixel(
                RescaleRenderElement::from_element(dot, (0, 0).into(), 1.0),
            ));
        }
    }

    // Wallpaper — the very back of the stack. Dimmed in overview. During a
    // workspace switch it zooms out from 103% and settles, so even a switch
    // to an empty desk visibly moves (the alpha dip alone vanishes on dark
    // wallpapers — near-black toward near-black).
    if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        (0.0, 0.0),
        &surface.wallpaper,
        Some(wallpaper_alpha),
        None,
        None,
        Kind::Unspecified,
    ) {
        let zoom = match ws_progress.filter(|p| *p < 1.0) {
            Some(p) => 1.05 - 0.05 * ease_out(p) as f64,
            None => 1.0,
        };
        let osize = state.space.output_geometry(&surface.output)
            .map(|g| g.size)
            .unwrap_or_else(|| (1, 1).into());
        let center = smithay::utils::Point::<i32, smithay::utils::Physical>::from((
            osize.w / 2, osize.h / 2,
        ));
        elements.push(OutputRenderElements::Wallpaper(
            RescaleRenderElement::from_element(elem, center, zoom),
        ));
    }

    // ── frosted glass ────────────────────────────────────────────────────────
    // Only runs while something wants it (the menu is open). The desktop
    // part of the element stack (everything under the menu) is re-rendered
    // into a 1/4-size offscreen texture, gaussian-blurred in four separable
    // passes, and a rounded crop of the result is slid in directly beneath
    // each requesting surface. The 4x downscale does half the softening and
    // keeps the passes cheap enough for virgl.
    if !blur_rects.is_empty() {
        use smithay::backend::renderer::{Bind, Frame, Offscreen, Renderer as _, Texture};
        use smithay::backend::renderer::element::Element as _;
        const DOWN: i32 = 4;
        let out_size = state.space.output_geometry(&surface.output)
            .map(|g| g.size)
            .unwrap_or_else(|| (1, 1).into());
        let (qw, qh) = ((out_size.w / DOWN).max(1), (out_size.h / DOWN).max(1));
        let stale = blur_texs.as_ref()
            .map(|(a, _)| { let s = Texture::size(a); s.w != qw || s.h != qh })
            .unwrap_or(true);
        if stale {
            let a = renderer.create_buffer(smithay::backend::allocator::Fourcc::Abgr8888, (qw, qh).into());
            let b = renderer.create_buffer(smithay::backend::allocator::Fourcc::Abgr8888, (qw, qh).into());
            match (a, b) {
                (Ok(a), Ok(b)) => *blur_texs = Some((a, b)),
                (a, b) => {
                    tracing::warn!(a_err = a.is_err(), b_err = b.is_err(), "blur target alloc failed");
                    *blur_texs = None;
                }
            }
        }
        if let Some((texa, texb)) = blur_texs.as_mut() {
            let qsize = smithay::utils::Size::<i32, smithay::utils::Physical>::from((qw, qh));
            let full  = smithay::utils::Rectangle::from_size(qsize);
            let theme_clear = Color32F::new(
                theme.background[0], theme.background[1], theme.background[2], 1.0,
            );

            // Pass 0: desktop (elements below blur_mark are the menu itself
            // and the cursor — skipped) into texa, downscaled, back-to-front.
            let scene = (|| -> std::result::Result<(), smithay::backend::renderer::gles::GlesError> {
                let mut fb = renderer.bind(texa)?;
                let mut frame = renderer.render(&mut fb, qsize, Transform::Normal)?;
                frame.clear(theme_clear, &[full])?;
                for elem in elements[blur_mark..].iter().rev() {
                    let src = elem.src();
                    let dst = elem.geometry(scale);
                    let dst = smithay::utils::Rectangle::<i32, smithay::utils::Physical>::new(
                        (dst.loc.x / DOWN, dst.loc.y / DOWN).into(),
                        ((dst.size.w / DOWN).max(1), (dst.size.h / DOWN).max(1)).into(),
                    );
                    let _ = RenderElement::<GlesRenderer>::draw(elem, &mut frame, src, dst, &[full], &[], None);
                }
                let _ = frame.finish()?;
                Ok(())
            })();

            // Passes 1-4: separable gaussian, ping-pong, radius growing —
            // ends back in texa.
            let mut blurred = scene.is_ok();
            if blurred {
                let dirs: [(f32, f32); 4] = [
                    (1.0 / qw as f32, 0.0), (0.0, 1.0 / qh as f32),
                    (2.0 / qw as f32, 0.0), (0.0, 2.0 / qh as f32),
                ];
                for (i, dir) in dirs.iter().enumerate() {
                    let (from, to) = if i % 2 == 0 {
                        (texa.clone(), &mut *texb)
                    } else {
                        (texb.clone(), &mut *texa)
                    };
                    let pass = (|| -> std::result::Result<(), smithay::backend::renderer::gles::GlesError> {
                        let mut fb = renderer.bind(to)?;
                        let mut frame = renderer.render(&mut fb, qsize, Transform::Normal)?;
                        frame.override_default_tex_program(
                            blur_prog.clone(),
                            vec![Uniform::new("dir", [dir.0, dir.1])],
                        );
                        let elem = smithay::backend::renderer::element::texture::TextureRenderElement::from_static_texture(
                            smithay::backend::renderer::element::Id::new(),
                            ctx.clone(),
                            (0.0, 0.0),
                            from,
                            1,
                            Transform::Normal,
                            Some(1.0),
                            None,
                            None,
                            None,
                            Kind::Unspecified,
                        );
                        let src = smithay::backend::renderer::element::Element::src(&elem);
                        let res = RenderElement::<GlesRenderer>::draw(&elem, &mut frame, src, full, &[full], &[], None);
                        frame.clear_tex_program_override();
                        res?;
                        let _ = frame.finish()?;
                        Ok(())
                    })();
                    if let Err(e) = pass {
                        tracing::warn!(?e, "blur pass failed");
                        blurred = false;
                        break;
                    }
                }
            } else if let Err(e) = scene {
                tracing::warn!(?e, "blur scene pass failed");
            }

            // Rounded crops of the blurred desktop, one per requesting rect,
            // spliced in directly beneath the menu.
            if blurred {
                for (i, r) in blur_rects.iter().enumerate() {
                    let src = smithay::utils::Rectangle::<f64, smithay::utils::Logical>::new(
                        (r.loc.x as f64 / DOWN as f64, r.loc.y as f64 / DOWN as f64).into(),
                        (r.size.w as f64 / DOWN as f64, r.size.h as f64 / DOWN as f64).into(),
                    );
                    let inner = smithay::backend::renderer::element::texture::TextureRenderElement::from_static_texture(
                        smithay::backend::renderer::element::Id::new(),
                        ctx.clone(),
                        (r.loc.x as f64, r.loc.y as f64),
                        texa.clone(),
                        1,
                        Transform::Normal,
                        Some(1.0),
                        Some(src),
                        Some(r.size),
                        None,
                        Kind::Unspecified,
                    );
                    // 16px matches the menu card's CSS border-radius.
                    let patch = crate::render::BlurElement::new(inner, rounded_prog.clone(), 16.0);
                    elements.insert(blur_mark + i, OutputRenderElements::Blur(patch));
                }
            }
        }
    }

    // ── screenshot (IPC) ─────────────────────────────────────────────────────
    // Re-render the element stack (sans cursor) into an offscreen texture,
    // read it back, write PNG. One-shot; failures only log.
    if let Some(path) = state.screenshot.take() {
        use smithay::backend::renderer::{Bind, ExportMem, Frame, Offscreen, Renderer as _};
        use smithay::backend::renderer::element::Element as _;
        let out_size = state.space.output_geometry(&surface.output)
            .map(|g| g.size)
            .unwrap_or_else(|| (1, 1).into());
        let shot = (|| -> anyhow::Result<()> {
            let size_phys = smithay::utils::Size::<i32, smithay::utils::Physical>::from((out_size.w, out_size.h));
            let full = smithay::utils::Rectangle::from_size(size_phys);
            let mut tex: smithay::backend::renderer::gles::GlesTexture = renderer
                .create_buffer(smithay::backend::allocator::Fourcc::Abgr8888, (out_size.w, out_size.h).into())
                .map_err(|e| anyhow::anyhow!("create_buffer: {e:?}"))?;
            {
                let mut fb = renderer.bind(&mut tex)
                    .map_err(|e| anyhow::anyhow!("bind: {e:?}"))?;
                let mut frame = renderer.render(&mut fb, size_phys, Transform::Normal)
                    .map_err(|e| anyhow::anyhow!("render: {e:?}"))?;
                let bg = Color32F::new(theme.background[0], theme.background[1], theme.background[2], 1.0);
                frame.clear(bg, &[full]).map_err(|e| anyhow::anyhow!("clear: {e:?}"))?;
                for elem in elements[after_cursor..].iter().rev() {
                    let src = elem.src();
                    let dst = elem.geometry(scale);
                    let _ = RenderElement::<GlesRenderer>::draw(elem, &mut frame, src, dst, &[full], &[], None);
                }
                let _ = frame.finish().map_err(|e| anyhow::anyhow!("finish: {e:?}"))?;
            }
            let mapping = renderer.copy_texture(
                &tex,
                smithay::utils::Rectangle::from_size((out_size.w, out_size.h).into()),
                smithay::backend::allocator::Fourcc::Abgr8888,
            ).map_err(|e| anyhow::anyhow!("copy_texture: {e:?}"))?;
            let data = renderer.map_texture(&mapping)
                .map_err(|e| anyhow::anyhow!("map_texture: {e:?}"))?;
            let img = image::RgbaImage::from_raw(out_size.w as u32, out_size.h as u32, data.to_vec())
                .ok_or_else(|| anyhow::anyhow!("mapping size mismatch"))?;
            img.save(&path)?;
            Ok(())
        })();
        match shot {
            Ok(())  => tracing::info!(%path, "screenshot saved"),
            Err(e) => tracing::warn!(?e, %path, "screenshot failed"),
        }
    }

    // Keep the loop hot while animations are in flight.
    if anims_active {
        state.pending_redraw = true;
    }

    // Theme background (visible only where the wallpaper doesn't cover).
    let clear = Color32F::new(
        theme.background[0], theme.background[1], theme.background[2], 1.0,
    );
    let res = surface.compositor.render_frame(renderer, &elements, clear, FrameFlags::DEFAULT)
        .map_err(|e| anyhow::anyhow!("render_frame: {e:?}"))?;
    // Nothing changed → nothing to flip. Queuing anyway just earns an
    // EmptyFrame error from DRM (and a warn in the log) every idle frame.
    if !res.is_empty {
        surface.compositor.queue_frame(())
            .map_err(|e| anyhow::anyhow!("queue_frame: {e:?}"))?;
    }

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
        InputEvent::DeviceAdded { mut device } => {
            // Touchpad defaults (anything with tap support): tap-to-click,
            // tap-and-drag, natural scrolling. Mice keep traditional scroll.
            if device.config_tap_finger_count() > 0 {
                let _ = device.config_tap_set_enabled(true);
                let _ = device.config_tap_set_drag_enabled(true);
                let _ = device.config_scroll_set_natural_scroll_enabled(true);
            }
            tracing::info!(?device, "input device added");
        }
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
                    // vendi-lock: every key feeds the password buffer;
                    // nothing reaches clients or binds.
                    if data.vlock {
                        if key_state == smithay::backend::input::KeyState::Pressed {
                            use smithay::input::keyboard::xkb::keysyms;
                            match sym.raw() {
                                keysyms::KEY_Return | keysyms::KEY_KP_Enter => data.lock_submit(),
                                keysyms::KEY_Escape => {
                                    data.vlock_input.clear();
                                    data.pending_redraw = true;
                                }
                                keysyms::KEY_BackSpace => {
                                    data.vlock_input.pop();
                                    data.pending_redraw = true;
                                }
                                _ => {
                                    if let Some(c) = sym.key_char().filter(|c| !c.is_control()) {
                                        data.vlock_input.push(c);
                                        data.pending_redraw = true;
                                    }
                                }
                            }
                        }
                        return FilterResult::Intercept(None);
                    }
                    // Esc backs out of the overview without needing a bind.
                    if data.overview
                        && key_state == smithay::backend::input::KeyState::Pressed
                        && sym.raw() == smithay::input::keyboard::xkb::keysyms::KEY_Escape
                    {
                        return FilterResult::Intercept(Some(crate::input::Action::ToggleOverview));
                    }
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
            if state.vlock { return; }
            let Some(pointer) = state.seat.get_pointer() else { return };
            let delta_x = event.delta_x();
            let delta_y = event.delta_y();
            state.pointer_location += (delta_x, delta_y).into();
            clamp_pointer(state);
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
            if state.vlock { return; }
            let Some(pointer) = state.seat.get_pointer() else { return };
            let Some(output) = state.space.outputs().next().cloned() else { return };
            let Some(geo) = state.space.output_geometry(&output) else { return };
            let pos = event.position_transformed(geo.size);
            state.pointer_location = pos + geo.loc.to_f64();
            // Super+drag in progress: route motion into the drag, not the
            // client (QEMU and touchscreens deliver absolute motion — without
            // this, drags only worked on real mice).
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

        // ── click ────────────────────────────────────────────────────────────
        InputEvent::PointerButton { event } => {
            if state.vlock { return; }
            let Some(pointer) = state.seat.get_pointer() else { return };
            let bstate = event.state();
            const BTN_LEFT:  u32 = 0x110;
            const BTN_RIGHT: u32 = 0x111;

            // End an in-flight Super+drag on any release; the pick-up scale
            // eases back down where the window landed.
            if bstate == smithay::backend::input::ButtonState::Released && state.drag.is_some() {
                if let Some(drag) = state.drag.take() {
                    if !drag.resize {
                        state.drag_release = Some((drag.window, std::time::Instant::now()));
                    }
                }
                state.pending_redraw = true;
                return;
            }

            if bstate == smithay::backend::input::ButtonState::Pressed {
                // Overview: a click picks the cell under the cursor (focus +
                // zoom back), a miss just closes the overview. Clients never
                // see this press — the windows aren't really there.
                if state.overview {
                    let pos = state.pointer_location;
                    let layout = state.overview_layout();
                    let active = state.workspaces.active_id();
                    if let Some((window, _, ws)) = layout.cells.into_iter()
                        .find(|(_, cell, _)| cell.to_f64().contains(pos))
                    {
                        if ws != active {
                            // Cross-workspace pick: switching closes the
                            // overview itself and maps the window.
                            state.switch_workspace(ws);
                        } else {
                            state.toggle_overview();
                        }
                        state.focus_window(&window);
                        state.space.raise_element(&window, true);
                    } else if let Some((ws, _, _)) = layout.panels.into_iter()
                        .find(|(_, panel, _)| panel.to_f64().contains(pos))
                    {
                        if ws != active {
                            state.switch_workspace(ws);
                        } else {
                            state.toggle_overview();
                        }
                    } else {
                        state.toggle_overview();
                    }
                    return;
                }
                // Super+LeftDrag = move (a tiled window detaches in place and
                // follows the cursor); Super+RightDrag = resize (free-resize
                // floating, split-ratio drag on tiled).
                let logo = state.seat.get_keyboard()
                    .map(|k| k.modifier_state().logo)
                    .unwrap_or(false);
                let code = event.button_code();
                if logo && (code == BTN_LEFT || code == BTN_RIGHT) {
                    let pos = state.pointer_location;
                    let target = state.space.element_under(pos).map(|(w, _)| w.clone());
                    if let Some(window) = target {
                        let floating_rect = state.workspaces.active_ref().floating.iter()
                            .find(|(w, _)| w == &window)
                            .map(|(_, r)| *r);
                        let tiled = floating_rect.is_none()
                            && state.workspaces.active_ref().tree.contains(&window);
                        // Move/resize drags only act on floating windows —
                        // tiles stay tiled (Super+RightDrag still trades
                        // split ratios, KDE-style).
                        let drag = match (floating_rect, tiled, code) {
                            (Some(start_rect), _, _) => Some(crate::state::Drag {
                                window:      window.clone(),
                                resize:      code == BTN_RIGHT,
                                tile_resize: false,
                                start_ptr:   pos,
                                start_rect,
                                started:     std::time::Instant::now(),
                            }),
                            (None, true, BTN_RIGHT) => Some(crate::state::Drag {
                                window:      window.clone(),
                                resize:      true,
                                tile_resize: true,
                                start_ptr:   pos,
                                start_rect:  Default::default(),
                                started:     std::time::Instant::now(),
                            }),
                            _ => None,
                        };
                        if let Some(drag) = drag {
                            state.focus_window_at_cursor();
                            state.drag = Some(drag);
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
            if state.vlock { return; }
            use smithay::backend::input::{Axis, AxisSource};
            let Some(pointer) = state.seat.get_pointer() else { return };
            let source = event.source();
            // Wheel clicks often report ONLY v120 (discrete) — amount() comes
            // back None and an empty frame scrolls nothing (firefox). Fall
            // back to v120/120 * 15px per notch, and forward the discrete
            // value so clients that count notches get it too.
            let h = event.amount(Axis::Horizontal)
                .or_else(|| event.amount_v120(Axis::Horizontal).map(|d| d * 15.0 / 120.0))
                .unwrap_or(0.0);
            let v = event.amount(Axis::Vertical)
                .or_else(|| event.amount_v120(Axis::Vertical).map(|d| d * 15.0 / 120.0))
                .unwrap_or(0.0);
            let mut frame = AxisFrame::new(InputEventTrait::time_msec(&event)).source(source);
            if h != 0.0 {
                frame = frame.value(Axis::Horizontal, h);
                if let Some(d) = event.amount_v120(Axis::Horizontal) {
                    frame = frame.v120(Axis::Horizontal, d as i32);
                }
            }
            if v != 0.0 {
                frame = frame.value(Axis::Vertical, v);
                if let Some(d) = event.amount_v120(Axis::Vertical) {
                    frame = frame.v120(Axis::Vertical, d as i32);
                }
            }
            // Trackpad fingers lifting emit zero-amount events — those are
            // axis-stop markers (kinetic scroll cue), not empty frames.
            if source == AxisSource::Finger {
                if event.amount(Axis::Horizontal) == Some(0.0) { frame = frame.stop(Axis::Horizontal); }
                if event.amount(Axis::Vertical)   == Some(0.0) { frame = frame.stop(Axis::Vertical); }
            }
            pointer.axis(state, frame);
            pointer.frame(state);
        }

        // ── touchpad swipes ──────────────────────────────────────────────────
        // 3 fingers horizontal: workspace switch. 4 fingers vertical: swipe
        // up opens the launcher, down the actions menu.
        InputEvent::GestureSwipeBegin { event } => {
            if state.vlock { return; }
            use smithay::backend::input::GestureBeginEvent;
            state.swipe = Some((event.fingers(), 0.0, 0.0));
        }
        InputEvent::GestureSwipeUpdate { event } => {
            if state.vlock { return; }
            use smithay::backend::input::GestureSwipeUpdateEvent as _;
            if let Some((_, dx, dy)) = state.swipe.as_mut() {
                *dx += event.delta_x();
                *dy += event.delta_y();
            }
        }
        InputEvent::GestureSwipeEnd { event } => {
            if state.vlock { return; }
            use smithay::backend::input::GestureEndEvent;
            let Some((fingers, dx, dy)) = state.swipe.take() else { return };
            if event.cancelled() { return; }
            match fingers {
                3 if dx.abs() >= 120.0 && dx.abs() > dy.abs() => {
                    // Swiping left moves the viewport right → next workspace.
                    let forward = dx < 0.0;
                    if let Some(id) = state.workspaces.adjacent_id(forward) {
                        state.switch_workspace(id);
                    }
                }
                4 if dy.abs() >= 120.0 && dy.abs() > dx.abs() => {
                    if dy < 0.0 {
                        // Swipe up — Mission Control.
                        state.run_action(crate::input::Action::ToggleOverview);
                    } else {
                        let _ = std::process::Command::new("sh")
                            .arg("-c").arg("vendi-menu actions").spawn();
                    }
                }
                _ => {}
            }
        }

        _ => {}
    }
}

fn on_udev_event(event: UdevEvent, app: &mut UdevApp) {
    match event {
        UdevEvent::Added   { device_id, path } => tracing::info!(?device_id, ?path, "udev: device added"),
        // Monitor plugged/unplugged or changed resolution on a GPU we drive.
        UdevEvent::Changed { device_id } => {
            tracing::info!(?device_id, "udev: device changed — rescanning connectors");
            if let Ok(node) = DrmNode::from_dev_id(device_id) {
                // Events come for the card node; our device map is keyed by
                // the render node.
                let node = node.node_with_type(NodeType::Render)
                    .and_then(Result::ok)
                    .unwrap_or(node);
                if app.udev.drm_devices.contains_key(&node) {
                    rescan_connectors(app, node);
                }
            }
        }
        UdevEvent::Removed { device_id }       => tracing::info!(?device_id,        "udev: device removed"),
    }
}
