//! `wlr-screencopy-unstable-v1` — the screen-capture protocol every Wayland
//! recorder and screenshot tool speaks (wf-recorder, grim, OBS, the wlroots
//! desktop portal). vendiwm only ever rendered to its own DRM scanout, so
//! none of those worked; this wires the protocol to the same offscreen
//! render + read-back the IPC screenshot path already uses.
//!
//! Capture is two-phase: when a client asks to capture an output we answer
//! immediately with the buffer it should allocate (format + size), and when
//! it hands that buffer back via `copy` we stash the request and flag a
//! redraw. The backend fulfils stashed requests right after it renders the
//! matching output (see `fulfill_screencopy` in the udev backend), so the
//! captured frame is always the one that just hit the screen.
//!
//! Integrates through smithay's `dispatch2` framework: the per-resource user
//! data types implement `Dispatch2`/`GlobalDispatch2`, which the blanket
//! `delegate_dispatch2!(State)` turns into the real `Dispatch` impls.

use std::sync::atomic::{AtomicBool, Ordering};

use smithay::output::Output;
use smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
};
use smithay::reexports::wayland_server::{
    protocol::{wl_buffer::WlBuffer, wl_shm},
    Client, DataInit, DisplayHandle, New, Resource,
};
use smithay::utils::{Physical, Point, Rectangle, Size};
use smithay::wayland::{Dispatch2, GlobalDispatch2};

use crate::state::State;

/// A capture request waiting for the next render of its output. The backend
/// drains these in `fulfill_screencopy`.
pub struct PendingScreencopy {
    pub frame: ZwlrScreencopyFrameV1,
    pub buffer: WlBuffer,
    pub output: Output,
    pub overlay_cursor: bool,
    pub region: Rectangle<i32, Physical>,
    pub with_damage: bool,
}

/// Advertise the manager global. Version 3 = dmabuf hints + `buffer_done`;
/// we only ever offer shm, but answering at v3 keeps modern clients happy.
pub fn init(dh: &DisplayHandle) {
    dh.create_global::<State, ZwlrScreencopyManagerV1, _>(3, ManagerGlobalData);
}

// ── manager global ──────────────────────────────────────────────────────────

pub struct ManagerGlobalData;
pub struct ManagerData;

impl GlobalDispatch2<ZwlrScreencopyManagerV1, State> for ManagerGlobalData {
    fn bind(
        &self,
        _state: &mut State,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrScreencopyManagerV1>,
        data_init: &mut DataInit<'_, State>,
    ) {
        data_init.init(resource, ManagerData);
    }
}

impl Dispatch2<ZwlrScreencopyManagerV1, State> for ManagerData {
    fn request(
        &self,
        _state: &mut State,
        _client: &Client,
        _mgr: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        use zwlr_screencopy_manager_v1::Request;
        match request {
            Request::CaptureOutput { frame, overlay_cursor, output } => {
                let out = Output::from_resource(&output);
                let (ow, oh) = out
                    .as_ref()
                    .and_then(|o| o.current_mode())
                    .map(|m| (m.size.w, m.size.h))
                    .unwrap_or((0, 0));
                make_frame(data_init, frame, out, overlay_cursor != 0,
                           Rectangle::new(Point::from((0, 0)), Size::from((ow, oh))));
            }
            Request::CaptureOutputRegion {
                frame, overlay_cursor, output, x, y, width, height,
            } => {
                let out = Output::from_resource(&output);
                make_frame(data_init, frame, out, overlay_cursor != 0,
                           Rectangle::new(Point::from((x, y)),
                                          Size::from((width.max(0), height.max(0)))));
            }
            Request::Destroy => {}
            _ => {}
        }
    }
}

/// Create the frame object and immediately tell the client which buffer to
/// allocate. We hand back Abgr8888 (the read-back's native byte order) at the
/// region's size — grim/wf-recorder convert from there.
fn make_frame(
    data_init: &mut DataInit<'_, State>,
    frame: New<ZwlrScreencopyFrameV1>,
    output: Option<Output>,
    overlay_cursor: bool,
    region: Rectangle<i32, Physical>,
) {
    let data = FrameData {
        output,
        overlay_cursor,
        region,
        used: AtomicBool::new(false),
    };
    let f = data_init.init(frame, data);
    let (w, h) = (region.size.w, region.size.h);
    if f.data::<FrameData>().map(|d| d.output.is_some()).unwrap_or(false) && w > 0 && h > 0 {
        f.buffer(wl_shm::Format::Abgr8888, w as u32, h as u32, (w * 4) as u32);
        if f.version() >= 3 {
            f.buffer_done();
        }
    } else {
        f.failed();
    }
}

// ── frame ─────────────────────────────────────────────────────────────────

pub struct FrameData {
    output: Option<Output>,
    overlay_cursor: bool,
    region: Rectangle<i32, Physical>,
    used: AtomicBool,
}

impl Dispatch2<ZwlrScreencopyFrameV1, State> for FrameData {
    fn request(
        &self,
        state: &mut State,
        _client: &Client,
        frame: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        use zwlr_screencopy_frame_v1::Request;
        let (buffer, with_damage) = match request {
            Request::Copy { buffer } => (buffer, false),
            Request::CopyWithDamage { buffer } => (buffer, true),
            Request::Destroy => return,
            _ => return,
        };
        // The protocol allows exactly one copy per frame.
        if self.used.swap(true, Ordering::SeqCst) {
            frame.post_error(
                zwlr_screencopy_frame_v1::Error::AlreadyUsed,
                "frame copied more than once",
            );
            return;
        }
        let Some(output) = self.output.clone() else {
            frame.failed();
            return;
        };
        state.pending_screencopy.push(PendingScreencopy {
            frame: frame.clone(),
            buffer,
            output,
            overlay_cursor: self.overlay_cursor,
            region: self.region,
            with_damage,
        });
        // Wake the render loop so the capture lands on the next frame.
        state.pending_redraw = true;
    }
}
