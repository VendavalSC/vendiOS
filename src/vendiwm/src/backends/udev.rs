// Udev backend — vendiwm running as the session compositor.
//
// Talks directly to DRM/KMS for output, libinput for input, libseat for VT
// management + secure DRM/input fd access. This is what runs on boot.
//
// Phase 1 (current): session + udev + libinput init, GPU discovery, connector
// enumeration, input event dispatch. Renders nothing yet — windows still get
// laid out by the layout tree but no pixels reach the screen.
//
// Phase 2 (next): DrmCompositor per output, damage-tracked frame rendering
// driven by DRM page-flip events.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use smithay::{
    backend::{
        drm::{DrmDevice, DrmDeviceFd, DrmNode, NodeType},
        egl::{EGLContext, EGLDisplay, context::ContextPriority},
        input::{
            AbsolutePositionEvent, Event as InputEventTrait, InputEvent, KeyboardKeyEvent,
            PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
        },
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::gles::GlesRenderer,
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::{UdevBackend, UdevEvent, all_gpus, primary_gpu},
    },
    input::{
        keyboard::FilterResult,
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    reexports::{
        calloop::EventLoop,
        drm::control::{Device as ControlDevice, connector},
        gbm::{Device as GbmDevice},
        input::Libinput,
        rustix::fs::OFlags,
        wayland_server::Display,
    },
    utils::{DeviceFd, SERIAL_COUNTER},
};

use crate::state::State;

/// Top-level event-loop data — gives every calloop callback &mut access to
/// both the wayland state and the udev/DRM bits.
pub struct UdevApp {
    pub state: State,
    pub udev:  UdevData,
}

pub fn run() -> Result<()> {
    let mut event_loop: EventLoop<UdevApp> = EventLoop::try_new().context("calloop event loop")?;
    let loop_handle = event_loop.handle();

    let _display: Display<State> = Display::new().context("create wayland Display")?;

    // 1. Open seat from logind. notifier reports VT switch / pause / resume.
    let (session, notifier) = LibSeatSession::new()
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

    // 5. Open primary GPU. Rendering (mode + DrmCompositor + present loop)
    //    is the next phase — currently we just init the renderer + enumerate
    //    connectors so a future render pass has what it needs.
    if let Err(e) = udev.open_drm_device(&primary_gpu_path) {
        tracing::warn!(?e, "failed to open primary GPU");
    }

    // 6. Build the wayland State — same globals as the winit backend, but
    //    Output/Space population happens per-connector in phase 2.
    let state = build_state(&_display)?;
    let mut app = UdevApp { state, udev };

    // 7. Wire calloop event sources.
    loop_handle.insert_source(libinput_backend, move |event, _, app: &mut UdevApp| {
        on_libinput_event(event, app);
    }).map_err(|e| anyhow::anyhow!("insert libinput source: {e:?}"))?;

    loop_handle.insert_source(notifier, move |event, _, app: &mut UdevApp| {
        on_session_event(event, app);
    }).map_err(|e| anyhow::anyhow!("insert session source: {e:?}"))?;

    loop_handle.insert_source(udev_backend, move |event, _, app: &mut UdevApp| {
        on_udev_event(event, app);
    }).map_err(|e| anyhow::anyhow!("insert udev source: {e:?}"))?;

    tracing::info!("vendiwm udev backend running. Press Ctrl+C to exit.");
    event_loop.run(Duration::from_millis(16), &mut app, |_app| {
        // Per-tick: phase 2 schedules redraws + IPC poll here.
    }).context("run event loop")?;

    Ok(())
}

fn build_state(display: &Display<State>) -> Result<State> {
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
    let dh = display.handle();

    let compositor_state     = CompositorState::new::<State>(&dh);
    let xdg_shell_state      = XdgShellState::new::<State>(&dh);
    let shm_state            = ShmState::new::<State>(&dh, Vec::new()); // formats added once renderer is ready
    let data_device_state    = DataDeviceState::new::<State>(&dh);
    let output_manager_state = OutputManagerState::new_with_xdg_output::<State>(&dh);
    let layer_shell_state    = WlrLayerShellState::new::<State>(&dh);
    let dmabuf_state         = DmabufState::new();

    let mut seat_state = SeatState::new();
    let seat = seat_state.new_wl_seat(&dh, "vendi-seat-0");

    let config = crate::config::Config::load().unwrap_or_else(|e| {
        tracing::warn!(?e, "config load failed; using empty keybinds");
        crate::config::Config { keybinds: Default::default() }
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
        layout:                 crate::layout::Tree::new(),
        config,
        pointer_location:       (0.0, 0.0).into(),
        pending_dmabuf_imports: Vec::new(),
        pending_ipc_events:     Vec::new(),
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
    pub _drm:       DrmDevice,
    pub _gbm:       GbmDevice<DrmDeviceFd>,
    pub _renderer:  GlesRenderer,
    pub _gpu_path:  PathBuf,
    pub connectors: Vec<connector::Info>,
}

impl UdevData {
    fn open_drm_device(&mut self, path: &PathBuf) -> Result<()> {
        // 1. Open the device fd via libseat (rev'd up with DRM master).
        let fd = self.session.open(path,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
        ).map_err(|e| anyhow::anyhow!("session.open {path:?}: {e:?}"))?;
        let device_fd = DrmDeviceFd::new(DeviceFd::from(fd));

        // 2. DrmDevice — atomic KMS in modern mode.
        let (drm, _notifier) = DrmDevice::new(device_fd.clone(), true)
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
        let renderer = unsafe { GlesRenderer::new(egl_context) }
            .map_err(|e| anyhow::anyhow!("GlesRenderer::new: {e:?}"))?;

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

        let node = DrmNode::from_path(path)
            .map_err(|e| anyhow::anyhow!("DrmNode::from_path: {e:?}"))?;
        self.drm_devices.insert(node, DeviceState {
            _drm: drm,
            _gbm: gbm,
            _renderer: renderer,
            _gpu_path: path.clone(),
            connectors,
        });
        Ok(())
    }
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
                    let sym = handle.modified_sym();
                    match crate::input::handle(&data.config, sym.raw(), key_state, mods) {
                        Some(a) => FilterResult::Intercept(Some(a)),
                        None    => FilterResult::Forward,
                    }
                },
            );
            if let Some(Some(act)) = action {
                let layout_changed = matches!(
                    &act,
                    crate::input::Action::Close
                    | crate::input::Action::FocusNext
                    | crate::input::Action::FocusPrev,
                );
                if state.run_action(act) {
                    tracing::info!("quit action received");
                    // TODO: signal calloop event loop to exit.
                }
                if layout_changed { state.relayout(); }
            }
        }

        // ── pointer motion (relative — typical of mice) ──────────────────────
        InputEvent::PointerMotion { event } => {
            let Some(pointer) = state.seat.get_pointer() else { return };
            let delta_x = event.delta_x();
            let delta_y = event.delta_y();
            state.pointer_location += (delta_x, delta_y).into();
            // Clamp to first output's geometry so the cursor can't escape.
            if let Some(output) = state.space.outputs().next().cloned() {
                if let Some(geo) = state.space.output_geometry(&output) {
                    let max_x = (geo.loc.x + geo.size.w) as f64;
                    let max_y = (geo.loc.y + geo.size.h) as f64;
                    state.pointer_location.x = state.pointer_location.x.clamp(geo.loc.x as f64, max_x);
                    state.pointer_location.y = state.pointer_location.y.clamp(geo.loc.y as f64, max_y);
                }
            }
            let location = state.pointer_location;
            let under = state.surface_under(location).map(|(s, p)| (s.into(), p));
            pointer.motion(state, under, &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time:   InputEventTrait::time_msec(&event),
            });
            pointer.frame(state);
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
        }

        // ── click ────────────────────────────────────────────────────────────
        InputEvent::PointerButton { event } => {
            let Some(pointer) = state.seat.get_pointer() else { return };
            let bstate = event.state();
            if bstate == smithay::backend::input::ButtonState::Pressed {
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

        _ => {}
    }
}

fn on_session_event(event: SessionEvent, _app: &mut UdevApp) {
    match event {
        SessionEvent::PauseSession    => tracing::info!("session paused (VT switched away)"),
        SessionEvent::ActivateSession => tracing::info!("session activated (VT switched in)"),
    }
}

fn on_udev_event(event: UdevEvent, _app: &mut UdevApp) {
    match event {
        UdevEvent::Added   { device_id, path } => tracing::info!(?device_id, ?path, "udev: device added"),
        UdevEvent::Changed { device_id }       => tracing::info!(?device_id,        "udev: device changed"),
        UdevEvent::Removed { device_id }       => tracing::info!(?device_id,        "udev: device removed"),
    }
}
