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
        input::InputEvent,
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::gles::GlesRenderer,
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::{UdevBackend, UdevEvent, all_gpus, primary_gpu},
    },
    reexports::{
        calloop::EventLoop,
        drm::control::{Device as ControlDevice, connector},
        gbm::{Device as GbmDevice},
        input::Libinput,
        rustix::fs::OFlags,
        wayland_server::Display,
    },
    utils::DeviceFd,
};

use crate::state::State;

pub fn run() -> Result<()> {
    let mut event_loop: EventLoop<UdevData> = EventLoop::try_new().context("calloop event loop")?;
    let loop_handle  = event_loop.handle();

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

    let mut data = UdevData {
        seat_name: seat_name.clone(),
        session,
        primary_gpu: primary_gpu_node,
        drm_devices: HashMap::new(),
    };

    // 5. Open the primary GPU and probe its connectors. Surfaces / rendering
    //    come in phase 2 — for now we just log what's attached.
    if let Err(e) = data.open_drm_device(&primary_gpu_path) {
        tracing::warn!(?e, "failed to open primary GPU");
    }

    // 6. Wire calloop event sources.
    loop_handle.insert_source(libinput_backend, move |event, _, data| {
        on_libinput_event(event, data);
    }).map_err(|e| anyhow::anyhow!("insert libinput source: {e:?}"))?;

    loop_handle.insert_source(notifier, move |event, _, data| {
        on_session_event(event, data);
    }).map_err(|e| anyhow::anyhow!("insert session source: {e:?}"))?;

    loop_handle.insert_source(udev_backend, move |event, _, data| {
        on_udev_event(event, data);
    }).map_err(|e| anyhow::anyhow!("insert udev source: {e:?}"))?;

    tracing::info!("vendiwm udev backend running. Press Ctrl+C to exit.");
    event_loop.run(Duration::from_millis(16), &mut data, |_data| {
        // Per-tick callback — when we add the IPC + rendering loop they hook in here.
    }).context("run event loop")?;

    Ok(())
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

fn on_libinput_event(event: InputEvent<LibinputInputBackend>, _data: &mut UdevData) {
    // Phase 2 will route these through the existing input::handle() and
    // pointer.motion()/button()/axis() pipeline by holding &mut State alongside
    // UdevData. For now just log device hot-plug to confirm the backend is alive.
    match event {
        InputEvent::DeviceAdded { device }   => tracing::info!(?device, "input device added"),
        InputEvent::DeviceRemoved { device } => tracing::info!(?device, "input device removed"),
        _ => {}
    }
}

fn on_session_event(event: SessionEvent, data: &mut UdevData) {
    match event {
        SessionEvent::PauseSession => {
            tracing::info!("session paused (VT switched away)");
            // TODO: suspend libinput, release DRM master.
        }
        SessionEvent::ActivateSession => {
            tracing::info!("session activated (VT switched in)");
            // TODO: resume libinput, reclaim DRM master + force redraw.
            let _ = data;
        }
    }
}

fn on_udev_event(event: UdevEvent, _data: &mut UdevData) {
    match event {
        UdevEvent::Added   { device_id, path } => tracing::info!(?device_id, ?path, "udev: device added"),
        UdevEvent::Changed { device_id }       => tracing::info!(?device_id,        "udev: device changed"),
        UdevEvent::Removed { device_id }       => tracing::info!(?device_id,        "udev: device removed"),
    }
}
