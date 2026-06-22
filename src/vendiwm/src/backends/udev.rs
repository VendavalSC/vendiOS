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
                GlesPixelProgram, GlesRenderer, GlesTexProgram, GlesTexture, Uniform,
                UniformName, UniformType, element::PixelShaderElement,
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
    // Incoming wallpaper drawn through a growing circular mask (swap reveal).
    Reveal=crate::render::RevealElement,
    // Flat fill (session-start fade-in: black over everything, alpha → 0).
    Solid=smithay::backend::renderer::element::solid::SolidColorRenderElement,
}

type FrameElement = OutputRenderElements;

/// Freeze the desktop for the session-lock backdrop. The sharp texture
/// renders `elements[skip_sharp..]` (bar included — the first locked frame
/// must be pixel-identical to the live one); the blurred texture renders
/// `elements[skip_blur..]` (bar excluded — no notch ghost once the blob
/// detaches) at quarter res plus six separable gaussian passes.
fn capture_lock_backdrop(
    renderer: &mut GlesRenderer,
    elements: &[FrameElement],
    skip_sharp: usize,
    skip_blur: usize,
    bg: Color32F,
    out_size: smithay::utils::Size<i32, smithay::utils::Logical>,
    scale: smithay::utils::Scale<f64>,
    blur_prog: &GlesTexProgram,
) -> anyhow::Result<(GlesTexture, GlesTexture)> {
    use smithay::backend::renderer::{Bind, Frame, Offscreen, Renderer as _};
    use smithay::backend::renderer::element::Element as _;
    let ctx = smithay::backend::renderer::Renderer::context_id(renderer);
    let size_phys = smithay::utils::Size::<i32, smithay::utils::Physical>::from((out_size.w, out_size.h));
    let full = smithay::utils::Rectangle::from_size(size_phys);
    let mut sharp: GlesTexture = renderer
        .create_buffer(smithay::backend::allocator::Fourcc::Abgr8888, (out_size.w, out_size.h).into())
        .map_err(|e| anyhow::anyhow!("create sharp: {e:?}"))?;
    {
        let mut fb = renderer.bind(&mut sharp).map_err(|e| anyhow::anyhow!("bind: {e:?}"))?;
        let mut frame = renderer.render(&mut fb, size_phys, Transform::Normal)
            .map_err(|e| anyhow::anyhow!("render: {e:?}"))?;
        frame.clear(bg, &[full]).map_err(|e| anyhow::anyhow!("clear: {e:?}"))?;
        for elem in elements[skip_sharp..].iter().rev() {
            let src = elem.src();
            let dst = elem.geometry(scale);
            let _ = RenderElement::<GlesRenderer>::draw(elem, &mut frame, src, dst, &[full], &[], None);
        }
        let _ = frame.finish().map_err(|e| anyhow::anyhow!("finish: {e:?}"))?;
    }
    const DOWN: i32 = 4;
    let (qw, qh) = ((out_size.w / DOWN).max(1), (out_size.h / DOWN).max(1));
    let qsize = smithay::utils::Size::<i32, smithay::utils::Physical>::from((qw, qh));
    let qfull = smithay::utils::Rectangle::from_size(qsize);
    let mut texa: GlesTexture = renderer
        .create_buffer(smithay::backend::allocator::Fourcc::Abgr8888, (qw, qh).into())
        .map_err(|e| anyhow::anyhow!("create blur a: {e:?}"))?;
    let mut texb: GlesTexture = renderer
        .create_buffer(smithay::backend::allocator::Fourcc::Abgr8888, (qw, qh).into())
        .map_err(|e| anyhow::anyhow!("create blur b: {e:?}"))?;
    // Quarter-res scene pass, bar-free (does half the softening).
    {
        let mut fb = renderer.bind(&mut texa).map_err(|e| anyhow::anyhow!("bind: {e:?}"))?;
        let mut frame = renderer.render(&mut fb, qsize, Transform::Normal)
            .map_err(|e| anyhow::anyhow!("render: {e:?}"))?;
        frame.clear(bg, &[qfull]).map_err(|e| anyhow::anyhow!("clear: {e:?}"))?;
        for elem in elements[skip_blur..].iter().rev() {
            let src = elem.src();
            let dst = elem.geometry(scale);
            let dst = smithay::utils::Rectangle::<i32, smithay::utils::Physical>::new(
                (dst.loc.x / DOWN, dst.loc.y / DOWN).into(),
                ((dst.size.w / DOWN).max(1), (dst.size.h / DOWN).max(1)).into(),
            );
            let _ = RenderElement::<GlesRenderer>::draw(elem, &mut frame, src, dst, &[qfull], &[], None);
        }
        let _ = frame.finish().map_err(|e| anyhow::anyhow!("finish: {e:?}"))?;
    }
    let dirs: [(f32, f32); 6] = [
        (1.0 / qw as f32, 0.0), (0.0, 1.0 / qh as f32),
        (2.0 / qw as f32, 0.0), (0.0, 2.0 / qh as f32),
        (3.0 / qw as f32, 0.0), (0.0, 3.0 / qh as f32),
    ];
    for (i, dir) in dirs.iter().enumerate() {
        let (from, to) = if i % 2 == 0 { (texa.clone(), &mut texb) } else { (texb.clone(), &mut texa) };
        let mut fb = renderer.bind(to).map_err(|e| anyhow::anyhow!("bind: {e:?}"))?;
        let mut frame = renderer.render(&mut fb, qsize, Transform::Normal)
            .map_err(|e| anyhow::anyhow!("render: {e:?}"))?;
        frame.override_default_tex_program(blur_prog.clone(), vec![Uniform::new("dir", [dir.0, dir.1])]);
        let elem = smithay::backend::renderer::element::texture::TextureRenderElement::from_static_texture(
            smithay::backend::renderer::element::Id::new(), ctx.clone(), (0.0, 0.0),
            from, 1, Transform::Normal, Some(1.0), None, None, None, Kind::Unspecified,
        );
        let src = smithay::backend::renderer::element::Element::src(&elem);
        let res = RenderElement::<GlesRenderer>::draw(&elem, &mut frame, src, qfull, &[qfull], &[], None);
        frame.clear_tex_program_override();
        res.map_err(|e| anyhow::anyhow!("blur draw: {e:?}"))?;
        let _ = frame.finish().map_err(|e| anyhow::anyhow!("finish: {e:?}"))?;
    }
    // Six passes end with the final write back in texa.
    Ok((sharp, texa))
}

/// Composite one frame of the wallpaper bloom into a FRESH offscreen texture:
/// the outgoing wallpaper fills the buffer, the incoming wallpaper is drawn on
/// top through the circular-reveal shader (a feathered disc of radius
/// `reveal_phys` centred at `center_phys`, both in physical px). The returned
/// texture is then pushed as an ordinary fullscreen TextureRenderElement.
///
/// Why offscreen: on the NVIDIA proprietary driver the DrmCompositor only
/// PRESENTS a frame when an element's geometry changes OR a genuinely new buffer
/// is submitted. The old in-place disc (same wallpaper buffer + a growing shader
/// uniform) rendered fine but never reached the screen — it froze then popped.
/// Allocating a brand-new texture every frame (like swww's fresh wl_buffer per
/// frame) gives the compositor a new buffer to flip, so the bloom presents on
/// every GPU. The buffer is full mode-size and short-lived (~42 frames / 700ms).
//
// NOTE: superseded by the cross-fade transition (the offscreen bloom did NOT
// present on NVIDIA after all). Kept for the disc shader / possible reuse.
#[allow(dead_code)]
fn render_bloom(
    renderer: &mut GlesRenderer,
    old: &MemoryRenderBuffer,
    new: &MemoryRenderBuffer,
    reveal_prog: &GlesTexProgram,
    center_phys: (f32, f32),
    reveal_phys: f32,
    alpha: f32,
    wp_src: Option<smithay::utils::Rectangle<f64, smithay::utils::Logical>>,
    mode_size: smithay::utils::Size<i32, smithay::utils::Physical>,
) -> anyhow::Result<GlesTexture> {
    use smithay::backend::renderer::{Bind, Frame, Offscreen, Renderer as _};
    use smithay::backend::renderer::element::Element as _;
    // Offscreen pass works in physical px at scale 1 (the buffer is the native
    // mode size); the result is later drawn to the real output at its own scale.
    let scale1 = smithay::utils::Scale::from(1.0);
    let full = smithay::utils::Rectangle::from_size(mode_size);
    let dest = smithay::utils::Size::<i32, smithay::utils::Logical>::from((mode_size.w, mode_size.h));
    // Build the two source elements first — from_buffer needs &mut renderer,
    // which the bound frame below would otherwise hold exclusively.
    let old_elem = MemoryRenderBufferRenderElement::from_buffer(
        renderer, (0.0, 0.0), old, Some(alpha), wp_src, Some(dest), Kind::Unspecified,
    ).map_err(|e| anyhow::anyhow!("old elem: {e:?}"))?;
    let new_inner = MemoryRenderBufferRenderElement::from_buffer(
        renderer, (0.0, 0.0), new, Some(alpha), wp_src, Some(dest), Kind::Unspecified,
    ).map_err(|e| anyhow::anyhow!("new elem: {e:?}"))?;
    let reveal_elem = crate::render::RevealElement::new(
        new_inner, reveal_prog.clone(), center_phys, reveal_phys,
    );
    let old_src = old_elem.src();
    let old_dst = old_elem.geometry(scale1);
    let new_src = reveal_elem.src();
    let new_dst = reveal_elem.geometry(scale1);
    let mut tex: GlesTexture = renderer
        .create_buffer(smithay::backend::allocator::Fourcc::Abgr8888, (mode_size.w, mode_size.h).into())
        .map_err(|e| anyhow::anyhow!("create bloom: {e:?}"))?;
    {
        let mut fb = renderer.bind(&mut tex).map_err(|e| anyhow::anyhow!("bind: {e:?}"))?;
        let mut frame = renderer.render(&mut fb, mode_size, Transform::Normal)
            .map_err(|e| anyhow::anyhow!("render: {e:?}"))?;
        frame.clear(Color32F::new(0.0, 0.0, 0.0, 1.0), &[full]).map_err(|e| anyhow::anyhow!("clear: {e:?}"))?;
        // Outgoing wallpaper, full and opaque underneath.
        RenderElement::<GlesRenderer>::draw(&old_elem, &mut frame, old_src, old_dst, &[full], &[], None)
            .map_err(|e| anyhow::anyhow!("old draw: {e:?}"))?;
        // Incoming wallpaper through the growing disc.
        RenderElement::<GlesRenderer>::draw(&reveal_elem, &mut frame, new_src, new_dst, &[full], &[], None)
            .map_err(|e| anyhow::anyhow!("reveal draw: {e:?}"))?;
        let _ = frame.finish().map_err(|e| anyhow::anyhow!("finish: {e:?}"))?;
    }
    Ok(tex)
}

/// Render `elements[skip..]` (bottom-up) into an offscreen texture, read it
/// back, write a PNG. Shared by the normal and locked screenshot paths.
fn save_screenshot(
    renderer: &mut GlesRenderer,
    elements: &[FrameElement],
    skip: usize,
    bg: Color32F,
    out_size: smithay::utils::Size<i32, smithay::utils::Logical>,
    scale: smithay::utils::Scale<f64>,
    path: &str,
) -> anyhow::Result<()> {
    use smithay::backend::renderer::{Bind, ExportMem, Frame, Offscreen, Renderer as _};
    use smithay::backend::renderer::element::Element as _;
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
        frame.clear(bg, &[full]).map_err(|e| anyhow::anyhow!("clear: {e:?}"))?;
        for elem in elements[skip..].iter().rev() {
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
    img.save(path)?;
    Ok(())
}

/// Service every pending `wlr-screencopy` request that targets `output`:
/// re-render the scene into an offscreen, read back the requested region, and
/// blit it into the client's shm buffer. Drains the matching entries.
fn fulfill_screencopy(
    renderer: &mut GlesRenderer,
    elements: &[FrameElement],
    after_cursor: usize,
    bg: Color32F,
    out_size: smithay::utils::Size<i32, smithay::utils::Logical>,
    scale: smithay::utils::Scale<f64>,
    output: &smithay::output::Output,
    pending: &mut Vec<crate::screencopy::PendingScreencopy>,
) {
    use smithay::reexports::wayland_server::Resource as _;
    let mut i = 0;
    while i < pending.len() {
        if pending[i].output.name() != output.name() {
            i += 1;
            continue;
        }
        let p = pending.remove(i);
        if !p.frame.is_alive() {
            continue;
        }
        match copy_screencopy_frame(renderer, elements, after_cursor, bg, out_size, scale, &p) {
            Ok(()) => tracing::debug!("screencopy frame served"),
            Err(e) => {
                tracing::warn!(?e, "screencopy frame failed");
                p.frame.failed();
            }
        }
    }
}

fn copy_screencopy_frame(
    renderer: &mut GlesRenderer,
    elements: &[FrameElement],
    after_cursor: usize,
    bg: Color32F,
    out_size: smithay::utils::Size<i32, smithay::utils::Logical>,
    scale: smithay::utils::Scale<f64>,
    p: &crate::screencopy::PendingScreencopy,
) -> anyhow::Result<()> {
    use smithay::backend::renderer::{Bind, ExportMem, Frame, Offscreen, Renderer as _};
    use smithay::backend::renderer::element::Element as _;
    use smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_frame_v1::Flags;

    // Cursor is the front of the element stack; including it = draw from 0.
    let skip = if p.overlay_cursor { 0 } else { after_cursor };

    // Clamp the requested region to the output we actually have.
    let rx = p.region.loc.x.clamp(0, out_size.w);
    let ry = p.region.loc.y.clamp(0, out_size.h);
    let rw = p.region.size.w.min(out_size.w - rx);
    let rh = p.region.size.h.min(out_size.h - ry);
    if rw <= 0 || rh <= 0 {
        anyhow::bail!("empty capture region");
    }

    let size_phys = smithay::utils::Size::<i32, smithay::utils::Physical>::from((out_size.w, out_size.h));
    let full = smithay::utils::Rectangle::from_size(size_phys);
    let mut tex: smithay::backend::renderer::gles::GlesTexture = renderer
        .create_buffer(smithay::backend::allocator::Fourcc::Abgr8888, (out_size.w, out_size.h).into())
        .map_err(|e| anyhow::anyhow!("create_buffer: {e:?}"))?;
    {
        let mut fb = renderer.bind(&mut tex).map_err(|e| anyhow::anyhow!("bind: {e:?}"))?;
        let mut frame = renderer.render(&mut fb, size_phys, Transform::Normal)
            .map_err(|e| anyhow::anyhow!("render: {e:?}"))?;
        frame.clear(bg, &[full]).map_err(|e| anyhow::anyhow!("clear: {e:?}"))?;
        for elem in elements[skip..].iter().rev() {
            let src = elem.src();
            let dst = elem.geometry(scale);
            let _ = RenderElement::<GlesRenderer>::draw(elem, &mut frame, src, dst, &[full], &[], None);
        }
        let _ = frame.finish().map_err(|e| anyhow::anyhow!("finish: {e:?}"))?;
    }

    // Read back only the requested region.
    let region = smithay::utils::Rectangle::<i32, smithay::utils::Buffer>::new(
        (rx, ry).into(), (rw, rh).into());
    let mapping = renderer.copy_texture(&tex, region, smithay::backend::allocator::Fourcc::Abgr8888)
        .map_err(|e| anyhow::anyhow!("copy_texture: {e:?}"))?;
    let pixels = renderer.map_texture(&mapping)
        .map_err(|e| anyhow::anyhow!("map_texture: {e:?}"))?
        .to_vec();

    // Blit into the client's shm buffer, honouring its stride.
    let src_stride = rw as usize * 4;
    smithay::wayland::shm::with_buffer_contents_mut(&p.buffer, |ptr, len, bdata| {
        let bstride = bdata.stride as usize;
        let rows = (bdata.height as usize).min(rh as usize);
        let cols = (bdata.width as usize).min(rw as usize) * 4;
        let off = bdata.offset as usize;
        for y in 0..rows {
            let dst_off = off + y * bstride;
            if dst_off + cols > len {
                break;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(
                    pixels.as_ptr().add(y * src_stride),
                    ptr.add(dst_off),
                    cols,
                );
            }
        }
    }).map_err(|e| anyhow::anyhow!("shm buffer access: {e:?}"))?;

    // The read-back is already top-row-first, so no y-invert.
    p.frame.flags(Flags::empty());
    if p.with_damage {
        p.frame.damage(0, 0, rw as u32, rh as u32);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    p.frame.ready((secs >> 32) as u32, (secs & 0xFFFF_FFFF) as u32, now.subsec_nanos());
    Ok(())
}

/// Concrete `DrmCompositor` parameterisation we use. `U=()` means we hand a
/// unit value to `queue_frame` (no per-frame presentation feedback userdata).
type GbmDrmCompositor = DrmCompositor<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    (),
    DrmDeviceFd,
>;

// The calloop event loop dispatches directly on `State`: the udev/DRM runtime
// is `state.udev` and the display handle is `state.display_handle`, so every
// callback reaches it all through the one `&mut State`. (XWayland forces this —
// `X11Wm::start_wm` needs a `LoopHandle<'static, State>`.)

pub fn run() -> Result<()> {
    let mut event_loop: EventLoop<State> = EventLoop::try_new().context("calloop event loop")?;
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
        dmabuf_global: None,
    };

    // 5. Open primary GPU. This brings up DRM/GBM/EGL/GlesRenderer and
    //    enumerates connectors. The notifier delivers VBlank events.
    let (device_state, drm_notifier) = udev.open_drm_device(&primary_gpu_path)
        .context("open primary GPU")?;
    let shm_formats: Vec<_> = device_state.renderer.shm_formats().collect();
    udev.drm_devices.insert(primary_gpu_node, device_state);

    // 6. Build the wayland State with the renderer's SHM formats. wl_drm
    //    binding for Mesa EGL clients happens next.
    let mut app = build_state(&display_handle, shm_formats)?;
    app.udev = Some(udev);
    // wlr-screencopy — lets wf-recorder/grim/OBS/portals capture the screen.
    crate::screencopy::init(&display_handle);

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
    if let Some(dev) = app.udev.as_mut().unwrap().drm_devices.get_mut(&primary_gpu_node) {
        match dev.renderer.egl_context().display().bind_wl_display(&display_handle) {
            Ok(_)  => tracing::info!("EGL hardware-acceleration enabled (wl_drm bound)"),
            Err(e) => tracing::warn!(?e, "failed to bind wl_display — EGL clients may not work"),
        }
    }

    // linux-dmabuf v4 with default feedback advertising the primary GPU's
    // render node. WITHOUT this, Mesa EGL clients (mpv, Firefox, games) can't
    // discover the device — they hit "failed to get driver name for fd -1" and
    // silently fall back to software rendering. wl_drm (bound above) is the
    // deprecated legacy path that modern Mesa no longer relies on; this global
    // is what actually gives clients hardware acceleration.
    {
        use smithay::wayland::dmabuf::DmabufFeedbackBuilder;
        let formats: Option<Vec<_>> = app.udev.as_mut().unwrap().drm_devices.get(&primary_gpu_node)
            .map(|dev| dev.renderer.egl_context().dmabuf_render_formats().iter().copied().collect());
        if let Some(formats) = formats {
            match DmabufFeedbackBuilder::new(primary_gpu_node.dev_id(), formats).build() {
                Ok(feedback) => {
                    let global = app.dmabuf_state
                        .create_global_with_default_feedback::<State>(&display_handle, &feedback);
                    app.udev.as_mut().unwrap().dmabuf_global = Some(global);
                    tracing::info!("linux-dmabuf v4 global up — clients can use the GPU");
                }
                Err(e) => tracing::warn!(?e, "dmabuf feedback build failed; clients stay software-only"),
            }
        }
    }

    // 9. Calloop sources.
    loop_handle.insert_source(libinput_backend, move |event, _, app: &mut State| {
        on_libinput_event(event, app);
    }).map_err(|e| anyhow::anyhow!("insert libinput source: {e:?}"))?;

    // VT switch / session pause+resume. We have to release DRM master and
    // suspend libinput when switching away, then re-take them on return —
    // otherwise the kernel can't switch VTs (we hold master) and on resume the
    // input devices stay dead.
    let mut session_libinput = libinput_context.clone();
    loop_handle.insert_source(session_notifier, move |event, _, app: &mut State| {
        match event {
            SessionEvent::PauseSession => {
                tracing::info!("session paused (VT switched away)");
                session_libinput.suspend();
                for dev in app.udev.as_mut().unwrap().drm_devices.values_mut() {
                    dev.drm.pause();
                }
            }
            SessionEvent::ActivateSession => {
                tracing::info!("session activated (VT switched in)");
                if let Err(e) = session_libinput.resume() {
                    tracing::warn!(?e, "libinput resume failed");
                }
                for dev in app.udev.as_mut().unwrap().drm_devices.values_mut() {
                    if let Err(e) = dev.drm.activate(false) {
                        tracing::warn!(?e, "drm activate failed");
                    }
                }
                app.pending_redraw = true;
            }
        }
    }).map_err(|e| anyhow::anyhow!("insert session source: {e:?}"))?;

    loop_handle.insert_source(udev_backend, move |event, _, app: &mut State| {
        on_udev_event(event, app);
    }).map_err(|e| anyhow::anyhow!("insert udev source: {e:?}"))?;

    // DRM page-flip / VBlank events drive rendering — one frame on each tick.
    loop_handle.insert_source(drm_notifier, move |event, _, app: &mut State| {
        match event {
            DrmEvent::VBlank(crtc) => {
                // Acknowledge the just-finished frame, then queue the next.
                if let Some(dev) = app.udev.as_mut().unwrap().drm_devices.get_mut(&primary_gpu_node) {
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
    loop_handle.insert_source(listening, move |stream, _, _app: &mut State| {
        if let Err(e) = dh_for_socket.insert_client(stream, Arc::new(ClientState::default())) {
            tracing::warn!(?e, "insert client failed");
        }
    }).map_err(|e| anyhow::anyhow!("insert socket source: {e:?}"))?;

    // Wayland client dispatch — wake on the Display fd, run handlers.
    loop_handle.insert_source(
        Generic::new(display, Interest::READ, CalloopMode::Level),
        |_, display, app: &mut State| {
            // SAFETY: the Generic source owns the Display for its lifetime
            // and we never drop or move it from inside the callback.
            unsafe {
                let _ = display.get_mut().dispatch_clients(&mut *app);
            }
            Ok(PostAction::Continue)
        },
    ).map_err(|e| anyhow::anyhow!("insert display source: {e:?}"))?;

    // XWayland: bring up the Xserver so X11-only apps (Discord/Electron, which
    // can't use vendiwm's native-Wayland path on NVIDIA) run. The server is a
    // wayland client of the socket above; once it's Ready we attach the X11
    // window manager (state.xwm) and export $DISPLAY so launchers reach it.
    #[cfg(feature = "xwayland")]
    {
        use smithay::xwayland::{XWayland, XWaylandEvent, X11Wm};
        use std::process::Stdio;
        match XWayland::spawn(
            &display_handle,
            None,
            std::iter::empty::<(String, String)>(),
            true,
            Stdio::null(),
            Stdio::null(),
            |_| (),
        ) {
            Ok((xwayland, xclient)) => {
                let xwm_loop = loop_handle.clone();
                let xwm_dh = display_handle.clone();
                let res = loop_handle.insert_source(xwayland, move |event, _, state: &mut State| {
                    match event {
                        XWaylandEvent::Ready { x11_socket, display_number } => {
                            match X11Wm::start_wm(
                                xwm_loop.clone(),
                                &xwm_dh,
                                x11_socket,
                                xclient.clone(),
                            ) {
                                Ok(wm) => {
                                    state.xwm = Some(wm);
                                    state.xdisplay = Some(display_number);
                                    let disp = format!(":{display_number}");
                                    // Children we spawn inherit our env; also push
                                    // DISPLAY into the dbus/systemd activation env so
                                    // bar- and dbus-launched apps pick it up too.
                                    unsafe { std::env::set_var("DISPLAY", &disp); }
                                    let _ = std::process::Command::new("dbus-update-activation-environment")
                                        .args(["--systemd", &format!("DISPLAY={disp}")]).spawn();
                                    let _ = std::process::Command::new("systemctl")
                                        .args(["--user", "import-environment", "DISPLAY"]).spawn();
                                    tracing::info!(display = %disp, "XWayland ready");
                                }
                                Err(e) => tracing::error!(?e, "X11Wm::start_wm failed"),
                            }
                        }
                        XWaylandEvent::Error => tracing::warn!("XWayland crashed on startup"),
                    }
                });
                if let Err(e) = res {
                    tracing::error!(?e, "insert XWayland source failed");
                }
            }
            Err(e) => tracing::error!(?e, "XWayland::spawn failed"),
        }
    }

    // Idle auto-lock: poll every few seconds; once input has been quiet for
    // longer than config.idle_lock_secs, fire `vendi-ctl lock` exactly once
    // (auto_lock_fired clears on the next input). 0 disables.
    {
        use smithay::reexports::calloop::timer::{Timer, TimeoutAction};
        const TICK: Duration = Duration::from_secs(5);
        loop_handle.insert_source(Timer::from_duration(TICK), |_, _, app: &mut State| {
            // A playing video / presentation holding an idle inhibitor keeps
            // the session awake — bump the clock so the countdown restarts
            // fresh once the inhibitor goes away.
            if app.idle_inhibited() {
                app.last_activity = std::time::Instant::now();
                return TimeoutAction::ToDuration(TICK);
            }
            // Reap a screensaver launcher that exited without mapping a window
            // (self-skipped on battery / no video, or mpv failed) so a later
            // unrelated window isn't mistaken for the screensaver.
            if app.screensaver.is_none() {
                if let Some(child) = app.screensaver_child.as_mut() {
                    if matches!(child.try_wait(), Ok(Some(_))) {
                        app.screensaver_child = None;
                    }
                }
            }
            let idle = app.last_activity.elapsed().as_secs();
            let lock_secs = app.config.idle_lock_secs;
            if lock_secs > 0
                && !app.auto_lock_fired
                && !app.is_locked()
                && !app.vlock
                && idle >= lock_secs
            {
                app.auto_lock_fired = true;
                tracing::info!(idle_secs = lock_secs, "idle auto-lock");
                if let Err(e) = std::process::Command::new("sh")
                    .arg("-c").arg("vendi-ctl lock").spawn()
                {
                    tracing::warn!(?e, "auto-lock spawn failed");
                }
            }
            // Video screensaver: fires before screen-off. The launcher itself
            // skips on battery / when no video is set, so the compositor can
            // arm it unconditionally. One spawn per idle stretch (input resets).
            let saver_secs = app.config.idle_screensaver_secs;
            if saver_secs > 0
                && !app.screensaver_fired
                && app.screensaver_child.is_none()
                && !app.is_locked()
                && !app.vlock
                && !app.screen_off
                && idle >= saver_secs
            {
                app.screensaver_fired = true;
                match std::process::Command::new("vendi-screensaver").arg("start").spawn() {
                    Ok(child) => {
                        app.screensaver_child = Some(child);
                        tracing::info!(idle_secs = saver_secs, "screensaver started");
                    }
                    Err(e) => tracing::warn!(?e, "screensaver spawn failed"),
                }
            }
            let off_secs = app.config.idle_screen_off_secs;
            if off_secs > 0 && !app.screen_off && idle >= off_secs {
                // No point playing video to a screen that's about to go dark.
                app.dismiss_screensaver();
                app.screen_off = true;
                sleep_outputs(app);
            }
            TimeoutAction::ToDuration(TICK)
        }).map_err(|e| anyhow::anyhow!("insert idle timer: {e:?}"))?;
    }

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
        app.space.refresh();
        app.popups.cleanup();

        // Pump the IPC server: deliver queued events, answer requests.
        for ev in app.pending_ipc_events.drain(..).collect::<Vec<_>>() {
            ipc.emit(ev);
        }
        ipc.poll(&mut *app);

        // A live reload may have changed a monitor's mode/refresh — reprogram
        // the DRM surface in place so Hz/resolution take effect without a
        // session restart (brief blackout during the modeset is expected).
        if app.pending_output_modes {
            app.pending_output_modes = false;
            apply_output_modes(app);
        }


        let pending: Vec<_> = app.pending_dmabuf_imports.drain(..).collect();
        if !pending.is_empty() {
            let udev = app.udev.as_mut().unwrap();
            let primary_gpu = udev.primary_gpu;
            if let Some(dev) = udev.drm_devices.get_mut(&primary_gpu) {
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
        if app.pending_redraw {
            app.pending_redraw = false;
            let primary_gpu = app.udev.as_ref().unwrap().primary_gpu;
            let crtcs: Vec<_> = app.udev.as_ref().unwrap().drm_devices.get(&primary_gpu)
                .map(|d| d.surfaces.keys().copied().collect())
                .unwrap_or_default();
            for crtc in crtcs {
                if let Err(e) = render_surface(app, primary_gpu, crtc) {
                    tracing::trace!(?e, "tick render_surface");
                }
            }
        }

        let _ = display_handle_tick.flush_clients();

        if app.quit_requested {
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
    // cursor-shape-v1: lets clients request named cursor shapes (hand, text,
    // wait…) which arrive via SeatHandler::cursor_image. Delegated by the
    // blanket delegate_dispatch2!(State); the GlobalId need not be retained.
    let _ = smithay::wayland::cursor_shape::CursorShapeManagerState::new::<State>(dh);
    let output_manager_state = OutputManagerState::new_with_xdg_output::<State>(dh);
    let layer_shell_state    = WlrLayerShellState::new::<State>(dh);
    let session_lock_state   = smithay::wayland::session_lock::SessionLockManagerState::new::<State, _>(dh, |_| true);
    let primary_selection_state = smithay::wayland::selection::primary_selection::PrimarySelectionState::new::<State>(dh);
    let data_control_state   = smithay::wayland::selection::wlr_data_control::DataControlState::new::<State, _>(
        dh, Some(&primary_selection_state), |_| true);
    let idle_inhibit_state   = smithay::wayland::idle_inhibit::IdleInhibitManagerState::new::<State>(dh);
    let xdg_decoration_state = smithay::wayland::shell::xdg::decoration::XdgDecorationState::new::<State>(dh);
    let viewporter_state     = smithay::wayland::viewporter::ViewporterState::new::<State>(dh);
    let fractional_scale_manager_state =
        smithay::wayland::fractional_scale::FractionalScaleManagerState::new::<State>(dh);
    let dmabuf_state         = DmabufState::new();

    let config = crate::config::Config::load().unwrap_or_else(|e| {
        tracing::warn!(?e, "config load failed; using empty keybinds");
        crate::config::Config { keybinds: Default::default(), keybinds_pretty: Default::default(), theme: Default::default(), idle_lock_secs: 0, idle_screen_off_secs: 0, idle_screensaver_secs: 0, kb_layout: "us".into(), kb_variant: String::new(), kb_options: String::new(), repeat_delay: 200, repeat_rate: 25, natural_scroll: None, tap_to_click: None, accel_speed: None, disable_while_typing: None, focus_follows_mouse: false, outputs: Vec::new(), window_rules: Vec::new() }
    });

    let mut seat_state = SeatState::new();
    let mut seat = seat_state.new_wl_seat(dh, "vendi-seat-0");
    let xkb = smithay::input::keyboard::XkbConfig {
        layout: &config.kb_layout,
        variant: &config.kb_variant,
        options: if config.kb_options.is_empty() { None } else { Some(config.kb_options.clone()) },
        ..Default::default()
    };
    seat.add_keyboard(xkb, config.repeat_delay, config.repeat_rate)
        .context("add keyboard to seat")?;
    let _ = seat.add_pointer();
    // No wl_touch: vendiOS emulates the mouse from the touchscreen (see
    // State::touch_down) so every desktop app — not just touch-aware ones —
    // gets tap=click, drag, long-press=right-click, and Super+drag to move.

    #[cfg(feature = "xwayland")]
    let xwayland_shell_state =
        smithay::wayland::xwayland_shell::XWaylandShellState::new::<State>(dh);

    Ok(State {
        display_handle: dh.clone(),
        pending_output_modes: false,
        #[cfg(feature = "xwayland")]
        xwayland_shell_state,
        #[cfg(feature = "xwayland")]
        xwm: None,
        #[cfg(feature = "xwayland")]
        xdisplay: None,
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
        idle_inhibitors:        Default::default(),
        xdg_decoration_state,
        viewporter_state,
        fractional_scale_manager_state,
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
        touch:                  None,
        touch_points:           Default::default(),
        touch_gesture:          None,
        drag_release:           None,
        swipe:                  None,
        overview:               false,
        overview_t:             std::time::Instant::now(),
        screenshot:             None,
        pending_screencopy:     Vec::new(),
        wallpaper_gen:          0,
        vlock: false,
        vlock_input: String::new(),
        vlock_fail: None,
        last_zone:              None,
        last_activity:          std::time::Instant::now(),
        auto_lock_fired:        false,
        screen_off:             false,
        screensaver_child:      None,
        screensaver:            None,
        screensaver_fired:      false,
        screensaver_t:          None,
        screensaver_closing:    None,
        open_anims:             Vec::new(),
        ws_anim:                None,
        geo_anims:              Vec::new(),
        closing:                Vec::new(),
        last_geos: HashMap::new(),
        config,
        pointer_location:       (0.0, 0.0).into(),
        cursor_status:          smithay::input::pointer::CursorImageStatus::default_named(),
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
    /// linux-dmabuf v4 global advertising the primary GPU to clients. Held so
    /// it stays alive for the session (clients need it for hardware EGL).
    pub dmabuf_global: Option<smithay::wayland::dmabuf::DmabufGlobal>,
}

pub struct DeviceState {
    pub drm:        DrmDevice,
    pub gbm:        GbmDevice<DrmDeviceFd>,
    pub renderer:   GlesRenderer,
    pub gpu_path:   PathBuf,
    /// NVIDIA proprietary driver: its DrmCompositor won't PRESENT a wallpaper
    /// bloom (an in-place disc OR a fresh-buffer-per-frame offscreen composite)
    /// — both render but never reach the screen. So NVIDIA gets the geometry
    /// slide instead; Mesa (AMD/Intel) gets the circular bloom.
    pub is_nvidia:  bool,
    pub connectors: Vec<connector::Info>,
    pub surfaces:   HashMap<crtc::Handle, SurfaceState>,
    /// XCursor-backed default pointer (plain arrow). Used while locked and as
    /// the fallback shape.
    pub cursor:     Cursor,
    /// Lazily-loaded themed cursors per requested shape (hand, I-beam, wait,
    /// resize…), so we don't re-parse the xcursor file every frame.
    pub named_cursors: HashMap<smithay::input::pointer::CursorIcon, Cursor>,
    /// Rounded-corner texture shader (applied per window element).
    pub rounded_prog: GlesTexProgram,
    /// Rounded border-ring pixel shader.
    pub border_prog:  GlesPixelProgram,
    /// Separable gaussian blur pass (frosted glass behind vendi-menu).
    pub blur_prog:    GlesTexProgram,
    /// Frosted-glass material shader (rounded crop of blurred desktop, lifted
    /// toward white so translucent windows read as bright glass, not dark).
    pub frost_prog:   GlesTexProgram,
    /// Circular wallpaper-reveal shader (wallpaper switch transition).
    pub reveal_prog:  GlesTexProgram,
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
    /// Outgoing wallpaper during a switch: (buffer, started, reveal center
    /// in output-local logical px). The new one blooms over it.
    pub old_wallpaper: Option<(MemoryRenderBuffer, std::time::Instant, (f32, f32))>,
    /// What `wallpaper` was built from (None = gradient) — reveals only play
    /// when this actually changes, not on every theme reload.
    pub wallpaper_src: Option<String>,
    /// ext-session-lock backdrop: the desktop frozen at lock time as
    /// (sharp full-res, blurred quarter-res, crossfade start). The start is
    /// None until the client's lock surface actually maps — the sharp frame
    /// (bar included, pixel-identical to the last live one) holds the screen
    /// until the blob is there to take over the notch, so nothing flickers.
    pub lock_backdrop: Option<(GlesTexture, GlesTexture, Option<std::time::Instant>)>,
    /// After unlock: the blurred backdrop melting away over the live desktop.
    pub lock_fade: Option<(GlesTexture, std::time::Instant)>,
    /// Session-start fade-in clock: set on this output's first rendered frame,
    /// then the desktop eases up from black over ~500ms so it doesn't snap in.
    pub start_fade: Option<std::time::Instant>,
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

        // Driver name decides the wallpaper transition (see is_nvidia doc).
        let is_nvidia = {
            use smithay::reexports::drm::Device as _;
            device_fd.get_driver().ok()
                .map(|d| d.name().to_string_lossy().to_lowercase().contains("nvidia"))
                .unwrap_or(false)
        };
        tracing::info!(?path, is_nvidia, "DRM driver probed");

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
        let frost_prog = renderer.compile_custom_texture_shader(
            crate::render::FROST_TEX_FRAG,
            &[
                UniformName::new("size",        UniformType::_2f),
                UniformName::new("radius",      UniformType::_1f),
                UniformName::new("lighten",     UniformType::_1f),
                UniformName::new("coord_off",   UniformType::_2f),
                UniformName::new("coord_scale", UniformType::_2f),
            ],
        ).map_err(|e| anyhow::anyhow!("compile frost shader: {e:?}"))?;
        let reveal_prog = renderer.compile_custom_texture_shader(
            crate::render::REVEAL_FRAG,
            &[
                UniformName::new("size",   UniformType::_2f),
                UniformName::new("center", UniformType::_2f),
                UniformName::new("reveal", UniformType::_1f),
            ],
        ).map_err(|e| anyhow::anyhow!("compile reveal shader: {e:?}"))?;

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
            is_nvidia,
            connectors,
            surfaces: HashMap::new(),
            cursor:   Cursor::load(),
            named_cursors: HashMap::new(),
            rounded_prog,
            border_prog,
            blur_prog,
            frost_prog,
            reveal_prog,
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
fn pick_mode(
    connector: &connector::Info,
    want: Option<(i32, i32, Option<u32>)>,
) -> Option<smithay::reexports::drm::control::Mode> {
    // Honour a configured resolution/refresh first: exact size match, then the
    // closest refresh if one was asked for.
    if let Some((w, h, hz)) = want {
        let mut best: Option<smithay::reexports::drm::control::Mode> = None;
        for m in connector.modes() {
            let (mw, mh) = m.size();
            if mw as i32 != w || mh as i32 != h { continue; }
            match hz {
                Some(want_hz) => {
                    if m.vrefresh() == want_hz { return Some(*m); }
                    let better = best.map_or(true, |b| {
                        (b.vrefresh() as i64 - want_hz as i64).abs()
                            > (m.vrefresh() as i64 - want_hz as i64).abs()
                    });
                    if better { best = Some(*m); }
                }
                None => best = Some(*m),
            }
        }
        if best.is_some() { return best; }
    }
    // Fall back to the connector's preferred mode (or the first listed).
    let idx = connector.modes().iter()
        .position(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
        .unwrap_or(0);
    connector.modes().get(idx).copied()
}

/// Reprogram any output whose configured mode/refresh differs from what its DRM
/// surface is currently running. Driven by `vendi-ctl output mode` → reload →
/// the `pending_output_modes` flag, so Hz/resolution changes hot-reload with
/// just a brief blackout (the modeset) instead of a session restart.
fn apply_output_modes(app: &mut State) {
    // Disjoint field borrows: the DRM devices and the output config are read
    // together inside the loop.
    let State { udev, config, .. } = &mut *app;
    let udev = udev.as_mut().unwrap();
    let mut changed = false;
    for device in udev.drm_devices.values_mut() {
        let crtcs: Vec<crtc::Handle> = device.surfaces.keys().copied().collect();
        for crtc in crtcs {
            let (conn_handle, cur_mode) = {
                let s = &device.surfaces[&crtc];
                (s.connector, s.mode)
            };
            let Ok(conn) = device.drm.get_connector(conn_handle, false) else { continue };
            let name = format!("{:?}-{}", conn.interface(), conn.interface_id());
            let want = config.output_cfg(&name).and_then(|c| c.mode);
            let Some(new_mode) = pick_mode(&conn, want) else { continue };
            // Compare size + refresh; nothing to do if already running it.
            if new_mode.size() == cur_mode.size()
                && new_mode.vrefresh() == cur_mode.vrefresh() {
                continue;
            }
            let surface = device.surfaces.get_mut(&crtc).unwrap();
            match surface.compositor.use_mode(new_mode) {
                Ok(()) => {
                    surface.mode = new_mode;
                    let wl_mode = WlMode::from(new_mode);
                    surface.output.change_current_state(Some(wl_mode), None, None, None);
                    surface.output.set_preferred(wl_mode);
                    // Re-render the wallpaper at the (possibly) new size.
                    let (mw, mh) = new_mode.size();
                    surface.wallpaper = crate::render::wallpaper_buffer(
                        mw as i32, mh as i32,
                        config.theme.wallpaper.as_deref(),
                        config.theme.background,
                        config.theme.accent,
                    );
                    smithay::desktop::layer_map_for_output(&surface.output).arrange();
                    tracing::info!(
                        connector = %name,
                        mode = ?(new_mode.size(), new_mode.vrefresh()),
                        "live mode change",
                    );
                    changed = true;
                }
                Err(e) => tracing::warn!(?e, connector = %name, "use_mode failed"),
            }
        }
    }
    if changed {
        app.relayout();
        app.pending_redraw = true;
    }
}

/// Bring up every connected connector at startup. Returns the CRTCs so the
/// caller can kick their first frames.
fn initial_surface_setup(app: &mut State, node: DrmNode) -> Result<Vec<crtc::Handle>> {
    let connected: Vec<connector::Info> = app.udev.as_mut().unwrap().drm_devices.get(&node)
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
    if let Some(geo) = app.space.outputs().next()
        .and_then(|o| app.space.output_geometry(o))
    {
        app.pointer_location =
            (geo.loc.x as f64 + geo.size.w as f64 / 2.0,
             geo.loc.y as f64 + geo.size.h as f64 / 2.0).into();
    }
    app.relayout();
    Ok(crtcs)
}

/// Bring up one connector: pick a mode, find a free CRTC, create a
/// DrmSurface + DrmCompositor + a smithay Output, and map the Output into
/// the Space to the right of everything already there.
fn connect_connector(
    app: &mut State,
    node: DrmNode,
    connector: &connector::Info,
) -> Result<crtc::Handle> {
    // Capture the Copy scalar up front, then split State into disjoint field
    // borrows so `device` (a deep borrow of state.udev) can coexist with the
    // space / config / display_handle the rest of the function touches.
    let wallpaper_gen = app.wallpaper_gen;
    let State { udev, config, space, display_handle, .. } = &mut *app;
    let udev = udev.as_mut().unwrap();
    let device = udev.drm_devices.get_mut(&node)
        .ok_or_else(|| anyhow::anyhow!("device not found: {node:?}"))?;

    // User arrangement for this connector (scale / position / mode), if any.
    let output_name = format!("{:?}-{}", connector.interface(), connector.interface_id());
    let ocfg = config.output_cfg(&output_name).cloned();

    let drm_mode = pick_mode(connector, ocfg.as_ref().and_then(|c| c.mode))
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
    // Position: configured, else to the right of everything already mapped.
    let next_x = space.outputs()
        .filter_map(|o| space.output_geometry(o))
        .map(|g| g.loc.x + g.size.w)
        .max()
        .unwrap_or(0);
    let pos = ocfg.as_ref().and_then(|c| c.position).unwrap_or((next_x, 0));
    // Scale: integer when whole (keeps the cheap path), fractional otherwise.
    let scale_val = ocfg.as_ref().and_then(|c| c.scale).unwrap_or(1.0);
    let scale = if (scale_val.fract()).abs() < 1e-6 {
        Scale::Integer(scale_val.max(1.0) as i32)
    } else {
        Scale::Fractional(scale_val)
    };
    let wl_mode = WlMode::from(drm_mode);
    output.change_current_state(
        Some(wl_mode),
        Some(Transform::Normal),
        Some(scale),
        Some(pos.into()),
    );
    output.set_preferred(wl_mode);
    let global = output.create_global::<State>(display_handle);
    space.map_output(&output, pos);

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
        config.theme.wallpaper.as_deref(),
        config.theme.background,
        config.theme.accent,
    );

    device.surfaces.insert(crtc, SurfaceState {
        output,
        compositor,
        wallpaper,
        wallpaper_gen,
        old_wallpaper: None,
        wallpaper_src: config.theme.wallpaper.clone(),
        lock_backdrop: None,
        lock_fade: None,
        start_fade: None,
        connector: connector.handle(),
        mode: drm_mode,
        global,
    });
    Ok(crtc)
}

/// React to a udev "changed" event: re-probe connectors, tear down surfaces
/// whose monitor vanished or switched resolution, bring up new ones, then
/// re-pack the remaining outputs left-to-right and keep the pointer inside.
fn rescan_connectors(app: &mut State, node: DrmNode) {
    // Snapshot the configured arrangement up front so the mode-stale check
    // below can match against the same mode connect_connector would pick
    // (otherwise a configured non-preferred mode looks "changed" every event
    // and thrashes the surface). Owned, so no borrow tangle with `device`.
    let cfg_outputs = app.config.outputs.clone();
    let cfg_mode = |name: &str| cfg_outputs.iter().find(|o| o.name == name).and_then(|o| o.mode);

    // 1. Re-probe.
    let (stale, fresh): (Vec<crtc::Handle>, Vec<connector::Info>) = {
        let Some(device) = app.udev.as_mut().unwrap().drm_devices.get_mut(&node) else { return };
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
                let want = cfg_mode(&s.output.name());
                let same_mode = info.and_then(|c| pick_mode(c, want)) == Some(s.mode);
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
        let Some(device) = app.udev.as_mut().unwrap().drm_devices.get_mut(&node) else { return };
        if let Some(s) = device.surfaces.remove(&crtc) {
            tracing::info!(output = %s.output.name(), "DRM output down");
            app.space.unmap_output(&s.output);
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

    // 4. Re-place outputs. Configured monitors go to their fixed position;
    //    the rest pack left-to-right in stable connector order so removals
    //    don't leave gaps the pointer could fall into.
    {
        // Snapshot (crtc, name, width) so we can consult the config (on
        // app) without holding the device borrow across the loop.
        let order: Vec<(crtc::Handle, String, i32)> = {
            let Some(device) = app.udev.as_mut().unwrap().drm_devices.get(&node) else { return };
            let mut v: Vec<_> = device.surfaces.iter()
                .map(|(c, s)| (*c, s.output.name(),
                    s.output.current_mode().map(|m| m.size.w).unwrap_or(0)))
                .collect();
            v.sort_by(|a, b| a.1.cmp(&b.1));
            v
        };
        let mut x = 0;
        for (crtc, name, w) in order {
            let cfg_pos = app.config.output_cfg(&name).and_then(|c| c.position);
            let pos = cfg_pos.unwrap_or((x, 0));
            if cfg_pos.is_none() { x += w; }
            if let Some(device) = app.udev.as_mut().unwrap().drm_devices.get(&node) {
                if let Some(s) = device.surfaces.get(&crtc) {
                    s.output.change_current_state(None, None, None, Some(pos.into()));
                    app.space.map_output(&s.output, pos);
                }
            }
        }
    }

    clamp_pointer(&mut *app);
    app.relayout();
    app.pending_redraw = true;

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

/// Idle screen-off: power every output down via DPMS (DrmCompositor::clear
/// disables the planes and sets DPMS off). Re-enabled by wake_outputs.
fn sleep_outputs(app: &mut State) {
    for device in app.udev.as_mut().unwrap().drm_devices.values_mut() {
        for surface in device.surfaces.values_mut() {
            if let Err(e) = surface.compositor.clear() {
                tracing::warn!(?e, "screen-off (clear) failed");
            }
        }
    }
    tracing::info!("displays off (idle DPMS)");
}

/// Wake every output back up: a fresh render + queue_frame re-enables DPMS
/// and restarts the VBlank cycle.
fn wake_outputs(app: &mut State) {
    let targets: Vec<(DrmNode, crtc::Handle)> = app.udev.as_mut().unwrap().drm_devices.iter()
        .flat_map(|(node, dev)| dev.surfaces.keys().map(move |c| (*node, *c)))
        .collect();
    for (node, crtc) in targets {
        if let Err(e) = render_surface(app, node, crtc) {
            tracing::warn!(?e, "wake render failed");
        }
    }
    tracing::info!("displays on (input woke them)");
}

/// Sloppy focus: if `focus-follows-mouse` is on, focus whatever window the
/// pointer is now over. Guarded so it only re-focuses on an actual change (not
/// every motion event) and never steals focus mid-drag, while locked, or over
/// empty space / the bar (focus_window_at_cursor no-ops when there's no window).
fn maybe_focus_follows_mouse(state: &mut State) {
    if !state.config.focus_follows_mouse || state.drag.is_some() || state.vlock {
        return;
    }
    let under = state.space.element_under(state.pointer_location).map(|(w, _)| w.clone());
    if under.is_some() && under != state.focused_window() {
        state.focus_window_at_cursor();
    }
}

fn render_surface(app: &mut State, node: DrmNode, crtc: crtc::Handle) -> Result<()> {
    // Displays are powered off (idle DPMS). Don't render or queue frames —
    // that would re-enable the output. Waking happens on input.
    if app.screen_off {
        return Ok(());
    }
    // ── whole-State work first ──────────────────────────────────────────────
    // These borrow all of State (&mut self method calls); do them BEFORE we
    // borrow the GPU device out of state.udev. After that point the body only
    // touches state via disjoint *field* accesses (state.config, state.space,
    // …), which coexist fine with the state.udev borrow `device` holds.

    // Screensaver: once the dismiss fade-out finishes, kill mpv for real.
    // While one is on screen, keep requesting redraws so the fade animates
    // smoothly even when mpv's (software) frames trickle in slowly.
    const SCREENSAVER_FADE_MS: f32 = 550.0;
    if app.screensaver_closing
        .map(|c| c.elapsed().as_secs_f32() * 1000.0 >= SCREENSAVER_FADE_MS)
        .unwrap_or(false)
    {
        app.dismiss_screensaver();
    }
    // Only pump extra redraws while a fade is actually animating. Doing it for
    // the whole screensaver lifetime busy-loops the compositor and starves
    // mpv's (software) decode — that's what made fullscreen playback choppy.
    // Steady-state playback rides mpv's own frame callbacks.
    let fade_active = app.screensaver_closing.is_some()
        || app.screensaver_t
            .map(|t| t.elapsed().as_secs_f32() * 1000.0 < SCREENSAVER_FADE_MS)
            .unwrap_or(false);
    if fade_active {
        app.pending_redraw = true;
    }
    // Touch long-press → right click. The 5s TICK timer is far too coarse, so
    // poll per-frame: while a finger is held (pending), keep redrawing so we
    // reach the ~450ms threshold, then touch_tick fires the synthetic click.
    app.touch_tick();
    if matches!(app.touch.as_ref().map(|t| t.phase), Some(crate::state::TouchPhase::Pending)) {
        app.pending_redraw = true;
    }
    // Pre-compute the two read-only whole-State queries the render body needs;
    // they can't run once `device` is borrowed from state.udev below. The
    // overview layout is gated by the same "in overview, or still within the
    // exit animation" window the body uses (see ov_layout / ov_exit).
    let overview_layout_pre = {
        const OVERVIEW_MS: f32 = 220.0;
        let now = std::time::Instant::now();
        let want = app.overview
            || (now.duration_since(app.overview_t).as_secs_f32() * 1000.0) < OVERVIEW_MS;
        if want { Some(app.overview_layout()) } else { None }
    };
    let is_locked_pre = app.is_locked();

    let state = &mut *app;
    let device = state.udev.as_mut().unwrap().drm_devices.get_mut(&node)
        .ok_or_else(|| anyhow::anyhow!("device not found"))?;

    // Split-borrow: hold renderer, surface, and cursor mutably at the same
    // time. They're distinct fields of DeviceState, so this is safe.
    let renderer = &mut device.renderer;
    let cursor   = &device.cursor;
    let named_cursors = &mut device.named_cursors;
    let rounded_prog = device.rounded_prog.clone();
    let border_prog  = device.border_prog.clone();
    let blur_prog    = device.blur_prog.clone();
    let frost_prog   = device.frost_prog.clone();
    let _reveal_prog  = device.reveal_prog.clone(); // (disc shader — kept for render_bloom)
    let _is_nvidia    = device.is_nvidia;           // (wallpaper transition is now a unified fade)
    let blur_texs     = &mut device.blur_texs;
    let tex_stash     = &mut device.tex_stash;
    let focus_anim    = &mut device.focus_anim;
    let last_tick     = &mut device.last_tick;
    let closing_anims = &mut device.closing_anims;
    let surface  = device.surfaces.get_mut(&crtc)
        .ok_or_else(|| anyhow::anyhow!("surface not found"))?;

    // Wallpaper changed over IPC: rebuild this output's buffer once, keep
    // the old one around for the bloom transition (reveal grows out of the
    // pointer — that's where the user clicked the thumbnail).
    if surface.wallpaper_gen != state.wallpaper_gen {
        let mode_size = surface.mode.size();
        // Owned copies so the buffer build doesn't borrow `state` while `surface`
        // (a borrow of state.udev) is held.
        let new_src = state.config.theme.wallpaper.clone();
        let bg = state.config.theme.background;
        let accent = state.config.theme.accent;
        let build = |src: Option<&str>| crate::render::wallpaper_buffer(
            mode_size.0 as i32, mode_size.1 as i32, src, bg, accent,
        );
        if new_src != surface.wallpaper_src {
            // Real wallpaper change → decode the new image and bloom it in from
            // the switch point.
            let out_geo = state.space.output_geometry(&surface.output).unwrap_or_default();
            let cx = (state.pointer_location.x - out_geo.loc.x as f64)
                .clamp(0.0, out_geo.size.w as f64) as f32;
            let cy = (state.pointer_location.y - out_geo.loc.y as f64)
                .clamp(0.0, out_geo.size.h as f64) as f32;
            let old = std::mem::replace(&mut surface.wallpaper, build(new_src.as_deref()));
            surface.old_wallpaper = Some((old, std::time::Instant::now(), (cx, cy)));
            surface.wallpaper_src = new_src;
        } else if new_src.is_none() {
            // Gradient theme (no image): bg/accent may have changed on a reload,
            // so rebuild the generated gradient — cheap, no image decode.
            surface.wallpaper = build(None);
        }
        // else: same image src — the buffer is already correct, so SKIP the
        // rebuild. A Dynamic theme regenerating its palette right after a
        // wallpaper switch bumps the gen again with the same image; re-decoding
        // that hi-res image every frame is what made the reveal stutter and snap
        // (it ate the 700ms wall-clock the bloom animates against).
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

    // Real output scale (HiDPI / fractional). The whole element stack below
    // is laid out in logical coords and converted to physical with this; a
    // few spots build physical points by hand and multiply by `sf` directly.
    let sf = surface.output.current_scale().fractional_scale();
    let scale = smithay::utils::Scale::from(sf);

    // ── session lock ─────────────────────────────────────────────────────────
    // Handled AFTER the desktop element stack is built (see below): the
    // desktop is frozen into a snapshot texture, blurred, and the lock
    // surface composes over the sharp→blurred crossfade. No live desktop
    // surface ever reaches a locked frame.

    // ── animation clocks ────────────────────────────────────────────────────
    // Open: fade + scale-in per window. Workspace switch: the whole desk
    // fades + settles. Eased with cubic ease-out; while anything is in
    // flight we keep pending_redraw set so the loop renders every tick.
    const OPEN_MS:  f32 = 260.0;
    const WS_MS:    f32 = 300.0;
    const MORPH_MS: f32 = 230.0;
    const DRAG_MS:  f32 = 120.0;
    // Frosted glass (blur on): the composite-alpha floor for windows so the
    // frost shows through even when the user hasn't cycled opacity, and how far
    // the frosted backdrop is lifted toward white so it reads bright, not dark.
    const FROST_WINDOW_ALPHA: f32 = 0.88;
    const FROST_LIGHTEN: f32 = 0.09;
    fn ease_out(t: f32) -> f32 { 1.0 - (1.0 - t).powi(3) }
    // Smooth accel + decel — used for the screensaver slide so neither the
    // entrance nor the exit yanks or crawls.
    fn ease_in_out(t: f32) -> f32 {
        if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
    }
    // Silkier decel than cubic — a longer, gentler tail so slides and layout
    // morphs glide to rest (the macOS feel) instead of braking late.
    fn ease_out_quint(t: f32) -> f32 { 1.0 - (1.0 - t).powi(5) }
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
            let e = ease_out_quint(p);
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
    // Pre-computed before the GPU device borrow (whole-State &self call); its
    // gate (in overview, or within the exit-animation window) matches here.
    let ov_layout = overview_layout_pre;
    let overview_cells: Vec<(smithay::desktop::Window, smithay::utils::Rectangle<i32, smithay::utils::Logical>)> =
        if state.overview {
            ov_layout.as_ref().map(|l| {
                l.cells.iter().map(|(w, r, _)| (w.clone(), *r)).collect()
            }).unwrap_or_default()
        } else {
            Vec::new()
        };
    let ov_e = ease_out_quint(
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
    // Bar elements tracked apart: the session-lock snapshot excludes them,
    // so the lock blob can replace the live center notch with no ghost
    // left in the blurred backdrop (and no flicker at the swap).
    let mut bar_layer_elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = Vec::new();
    // Logical rects that want a blurred-desktop slab behind them (the menu).
    let mut blur_rects: Vec<smithay::utils::Rectangle<i32, smithay::utils::Logical>> = Vec::new();
    // Translucent windows that want a frosted backdrop: (insert index into
    // `elements` — directly below the window, logical rect, corner radius).
    let mut blur_windows: Vec<(usize, smithay::utils::Rectangle<i32, smithay::utils::Logical>, f32)> = Vec::new();
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
            let sink = if layer.namespace().starts_with("vendibar") {
                &mut bar_layer_elems
            } else {
                &mut upper_layer_elems
            };
            sink.extend(
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
    // index 0 is drawn on top of everything else. Honour the client-requested
    // shape: a client-drawn surface, a hidden cursor, or a themed named shape
    // (hand/I-beam/wait/resize…); fall back to the plain arrow.
    if !locked {
        use smithay::input::pointer::{CursorIcon, CursorImageStatus};
        match &state.cursor_status {
            CursorImageStatus::Hidden => {}
            CursorImageStatus::Surface(surf) => {
                // Client renders its own cursor into this surface; the hotspot
                // lives in the surface's cursor attributes.
                let hotspot = smithay::wayland::compositor::with_states(surf, |states| {
                    states.data_map
                        .get::<smithay::input::pointer::CursorImageSurfaceData>()
                        .map(|d| d.lock().unwrap().hotspot)
                        .unwrap_or_default()
                });
                let loc = smithay::utils::Point::<i32, smithay::utils::Physical>::from((
                    ((state.pointer_location.x - out_loc.x as f64 - hotspot.x as f64) * sf) as i32,
                    ((state.pointer_location.y - out_loc.y as f64 - hotspot.y as f64) * sf) as i32,
                ));
                elements.extend(
                    smithay::backend::renderer::element::surface::render_elements_from_surface_tree::<
                        _, WaylandSurfaceRenderElement<GlesRenderer>,
                    >(renderer, surf, loc, scale, 1.0, Kind::Cursor)
                    .into_iter()
                    .map(OutputRenderElements::Layer),
                );
            }
            status => {
                let icon = match status {
                    CursorImageStatus::Named(i) => *i,
                    _ => CursorIcon::Default,
                };
                let cur = named_cursors.entry(icon)
                    .or_insert_with(|| crate::cursor::Cursor::load_icon(icon));
                let loc = smithay::utils::Point::<f64, smithay::utils::Physical>::from((
                    (state.pointer_location.x - out_loc.x as f64 - cur.hotspot.0 as f64) * sf,
                    (state.pointer_location.y - out_loc.y as f64 - cur.hotspot.1 as f64) * sf,
                ));
                if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
                    renderer, loc, &cur.buffer, None, None, None, Kind::Cursor,
                ) {
                    elements.push(OutputRenderElements::Memory(elem));
                }
            }
        }
    }
    // Screenshots skip everything up to here (i.e. the cursor).
    let after_cursor = elements.len();

    // Screensaver: the captured mpv window, full-bleed above the bar and every
    // desktop window (just under the cursor). No rounding or border — it's a
    // video, not a tile. Any input tears it down before the next frame.
    if let Some(ss) = state.screensaver.clone() {
        if smithay::utils::IsAlive::alive(&ss) {
            // Slide in from the top on appear, retract up on dismiss. We
            // animate GEOMETRY (not alpha) on purpose: the damage tracker
            // follows position changes, so the WHOLE screen animates — an alpha
            // fade only redrew self-updating widgets (the bar), never the
            // static desktop behind. ease-out both ways.
            let progress = if let Some(c) = state.screensaver_closing {
                // Exit: smooth accel + decel (ease-out alone yanks then crawls).
                ease_in_out((c.elapsed().as_secs_f32() * 1000.0 / SCREENSAVER_FADE_MS).min(1.0))
            } else if let Some(t0) = state.screensaver_t {
                // Entrance: snappy, settles into place.
                ease_out((t0.elapsed().as_secs_f32() * 1000.0 / SCREENSAVER_FADE_MS).min(1.0))
            } else {
                1.0
            };
            let h = ss.geometry().size.h as f32;
            let slide_y = if state.screensaver_closing.is_some() {
                -((h * progress) as i32)          // 0 → -h : retract off the top
            } else {
                -((h * (1.0 - progress)) as i32)  // -h → 0 : drop down into place
            };
            let render_loc = (smithay::utils::Point::<i32, smithay::utils::Logical>::from((0, slide_y))
                - ss.geometry().loc).to_physical_precise_round(scale);
            let surfaces: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                ss.render_elements(renderer, render_loc, scale, 1.0);
            for elem in surfaces.into_iter().rev() {
                elements.insert(after_cursor, OutputRenderElements::Layer(elem));
            }
        }
    }

    // Upper layers (Top/Overlay) → above windows but below the cursor.
    elements.extend(upper_layer_elems.into_iter().map(OutputRenderElements::Layer));
    // The bar sits just below the other upper layers (menus cover it). The
    // lock snapshot renders elements[after_bar..] — bar-free desktop, so the
    // lock blob can replace the live center notch with no ghost behind it.
    elements.extend(bar_layer_elems.into_iter().map(OutputRenderElements::Layer));
    let after_bar = elements.len();
    // Everything pushed from here down is "the desktop" — the blur pass at
    // the bottom of this function re-renders elements[blur_mark..] into an
    // offscreen target, and the frosted patches are inserted at this index
    // (directly beneath the menu, above all desktop content).
    let blur_mark = elements.len();

    // Close ghosts — above live windows (the dying window was usually on top).
    let ctx = smithay::backend::renderer::Renderer::context_id(renderer);
    for (eid, tex, geo, t) in closing_anims.iter().filter(|_| !locked) {
        let e = ease_out_quint((now.duration_since(*t).as_secs_f32() * 1000.0 / CLOSE_MS).min(1.0));
        let shrink = 1.0 - 0.15 * e as f64;
        let size = smithay::utils::Size::<i32, smithay::utils::Logical>::from((
            ((geo.size.w as f64 * shrink) as i32).max(1),
            ((geo.size.h as f64 * shrink) as i32).max(1),
        ));
        let loc = smithay::utils::Point::<f64, smithay::utils::Physical>::from((
            ((geo.loc.x - out_loc.x) as f64 + (geo.size.w - size.w) as f64 / 2.0) * sf,
            ((geo.loc.y - out_loc.y) as f64 + (geo.size.h - size.h) as f64 / 2.0) * sf,
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
    // The keyboard-focused surface (KbFocus → its wl_surface) — drives the
    // accent border ring on the active window.
    let focused_surf = state.seat.get_keyboard()
        .and_then(|k| k.current_focus())
        .and_then(|f| f.wl_surface());
    let fullscreen = state.workspaces.active_ref().fullscreen.clone();
    let stacked: Vec<_> = if locked { Vec::new() } else { state.space.elements().cloned().collect() };
    let mut live_ids: Vec<u32> = Vec::with_capacity(stacked.len());
    for window in stacked.iter().rev() {
        // The screensaver is in the space (for frame callbacks) but is drawn
        // by its own block above (sliding, above the bar). Skip it here so it
        // isn't ALSO rendered at its real position — that double-render is what
        // left a second, static copy behind the sliding one.
        if state.screensaver.as_ref() == Some(window) { continue; }
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
        // When blur is on, floor the window's composite alpha so the frosted
        // backdrop is always visible — blur is an independent on/off toggle, not
        // something you have to pair with a separate transparency mode. The
        // opacity-cycle (super+shift+o) still wins when set lower.
        let mut win_opa = crate::state::window_opacity(window, theme.opacity);
        if theme.blur && fullscreen.as_ref() != Some(window) {
            win_opa = win_opa.min(FROST_WINDOW_ALPHA);
        }
        let alpha = ws_alpha * open_t.map(ease_out).unwrap_or(1.0) * win_opa;
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
                let e = ease_out_quint((now.duration_since(*t).as_secs_f32() * 1000.0 / MORPH_MS).min(1.0));
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
                (((target.loc.x + target.size.w / 2) as f64) * sf).round() as i32,
                (((target.loc.y + target.size.h / 2) as f64) * sf).round() as i32,
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
            (((geo.loc.x + target.size.w / 2) as f64) * sf).round() as i32,
            (((geo.loc.y + target.size.h / 2) as f64) * sf).round() as i32,
        ));
        let off = (
            (((target.loc.x - geo.loc.x) as f64) * sf).round() as i32,
            (((target.loc.y - geo.loc.y) as f64) * sf).round() as i32,
        );
        for elem in surfaces {
            let rounded = crate::render::RoundedElement::new(elem, rounded_prog.clone(), radius);
            let morphed = RescaleRenderElement::from_element(rounded, anchor, morph_scale);
            let rescaled = RescaleRenderElement::from_element(morphed, content_center, scale_anim);
            elements.push(OutputRenderElements::Window(
                RelocateRenderElement::from_element(rescaled, off, Relocate::Relative),
            ));
        }

        // Frosted glass: when blur is on, splice a blurred slab of the desktop
        // directly behind every (non-fullscreen) window (index = just below the
        // window we only pushed). The composite-alpha floor above guarantees the
        // window is translucent enough for the frost to show, so this no longer
        // depends on detecting client-side alpha — which was unreliable and made
        // blur appear only in some opacity modes.
        if theme.blur && !is_fullscreen {
            blur_windows.push((elements.len(), target, radius));
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
    //
    // Wallpaper switch: the new image blooms over the old one — a feathered
    // disc growing from the switch point until it swallows the far corner.
    const REVEAL_MS: f32 = 900.0;
    let reveal = surface.old_wallpaper.as_ref().and_then(|(_, t0, c)| {
        let p = t0.elapsed().as_millis() as f32 / REVEAL_MS;
        (p < 1.0).then_some((ease_out(p), *c))
    });
    if reveal.is_none() { surface.old_wallpaper = None; } else { state.pending_redraw = true; }

    // Wallpaper sizing under scale: the buffer is built at physical mode size
    // (buffer_scale 1, so its logical size == the mode size). We draw it at the
    // logical output size (size = osize → physical = mode size, fills exactly),
    // and must crop from the FULL buffer (src = whole buffer) — otherwise the
    // source defaults to the smaller dest size and the wallpaper looks zoomed.
    let wp_mode = surface.output.current_mode().map(|m| m.size).unwrap_or_else(|| (1, 1).into());
    let wp_src = Some(smithay::utils::Rectangle::<f64, smithay::utils::Logical>::from_size(
        (wp_mode.w as f64, wp_mode.h as f64).into(),
    ));

    if let Some((p, _center)) = reveal {
        let osize = state.space.output_geometry(&surface.output)
            .map(|g| g.size)
            .unwrap_or_else(|| (1, 1).into());
        let center = smithay::utils::Point::<i32, smithay::utils::Physical>::from((
            ((osize.w as f64) * sf / 2.0) as i32, ((osize.h as f64) * sf / 2.0) as i32,
        ));
        // CROSS-FADE: the new wallpaper fades in (alpha 0→1) over the old one,
        // which sits full underneath — a soft dissolve, far less jarring than a
        // wipe for the frequent day→dusk→night transitions. The catch: a pure
        // alpha change on a static fullscreen buffer NEVER reaches the NVIDIA
        // display (the whole reason the old disc-bloom froze). So the incoming
        // wallpaper also rides a barely-perceptible zoom (1.03→1.0); the scale
        // is a geometry change every frame, which forces the page-flip, and the
        // alpha rides along. Reads as a gentle fade on every GPU.
        // NEW first = drawn on top (elements are front-to-back, index 0 topmost).
        let fade_a = (wallpaper_alpha * p as f32).clamp(0.0, 1.0);
        let zoom = 1.03 - 0.03 * p as f64;
        if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
            renderer, (0.0, 0.0), &surface.wallpaper, Some(fade_a), wp_src, Some(osize), Kind::Unspecified,
        ) {
            elements.push(OutputRenderElements::Wallpaper(
                RescaleRenderElement::from_element(elem, center, zoom)));
        }
        if let Some((old, ..)) = &surface.old_wallpaper {
            if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
                renderer, (0.0, 0.0), old, Some(wallpaper_alpha), wp_src, Some(osize), Kind::Unspecified,
            ) {
                elements.push(OutputRenderElements::Wallpaper(
                    RescaleRenderElement::from_element(elem, center, 1.0)));
            }
        }
    } else if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        (0.0, 0.0),
        &surface.wallpaper,
        Some(wallpaper_alpha),
        wp_src,
        Some(state.space.output_geometry(&surface.output).map(|g| g.size).unwrap_or_else(|| (1, 1).into())),
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
            ((osize.w as f64) * sf / 2.0) as i32, ((osize.h as f64) * sf / 2.0) as i32,
        ));
        elements.push(OutputRenderElements::Wallpaper(
            RescaleRenderElement::from_element(elem, center, zoom),
        ));
    }

    // ── session lock: lock surface over a frozen, blurring desktop ──────────
    // The desktop element stack above is complete but hasn't been rendered.
    // Freeze it into a snapshot (once per lock), then compose: cursor, the
    // client's lock surface, and a sharp→blurred crossfade of the snapshot.
    // No live desktop surface reaches a locked frame; the lock is confirmed
    // only after the first locked frame is queued, per ext-session-lock.
    if is_locked_pre {
        let out_size = state.space.output_geometry(&surface.output)
            .map(|g| g.size)
            .unwrap_or_else(|| (1, 1).into());
        if surface.lock_backdrop.is_none() {
            let bg = Color32F::new(theme.background[0], theme.background[1], theme.background[2], 1.0);
            match capture_lock_backdrop(renderer, &elements, after_cursor, after_bar, bg, out_size, scale, &blur_prog) {
                Ok((sharp, blurred)) => {
                    surface.lock_backdrop = Some((sharp, blurred, None));
                }
                Err(e) => tracing::warn!(?e, "lock backdrop capture failed"),
            }
        }
        let ctx = smithay::backend::renderer::Renderer::context_id(renderer);
        let mut lelems: Vec<FrameElement> = Vec::new();
        let pointer_phys = smithay::utils::Point::<f64, smithay::utils::Physical>::from((
            state.pointer_location.x - cursor.hotspot.0 as f64,
            state.pointer_location.y - cursor.hotspot.1 as f64,
        ));
        if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
            renderer, pointer_phys, &cursor.buffer, None, None, None, Kind::Cursor,
        ) {
            lelems.push(OutputRenderElements::Memory(elem));
        }
        let cursor_end = lelems.len();
        if let Some(lock) = &state.lock_surface {
            lelems.extend(
                smithay::backend::renderer::element::surface::render_elements_from_surface_tree::<
                    _, WaylandSurfaceRenderElement<GlesRenderer>,
                >(renderer, lock.wl_surface(), (0, 0), scale, 1.0, Kind::Unspecified)
                .into_iter()
                .map(OutputRenderElements::Layer),
            );
        }
        if let Some((sharp, blurred, started)) = &mut surface.lock_backdrop {
            // Hold the sharp frame until the blob has mapped, then blur in.
            if started.is_none() && state.lock_surface.is_some() {
                *started = Some(std::time::Instant::now());
            }
            const BLUR_IN_MS: f32 = 650.0;
            let t = started
                .map(|t0| (t0.elapsed().as_secs_f32() * 1000.0 / BLUR_IN_MS).min(1.0))
                .unwrap_or(0.0);
            let t = 1.0 - (1.0 - t).powi(3);
            if t < 1.0 {
                if started.is_some() {
                    state.pending_redraw = true;
                }
                let elem = smithay::backend::renderer::element::texture::TextureRenderElement::from_static_texture(
                    smithay::backend::renderer::element::Id::new(), ctx.clone(), (0.0, 0.0),
                    sharp.clone(), 1, Transform::Normal, Some(1.0 - t), None, None, None, Kind::Unspecified,
                );
                lelems.push(OutputRenderElements::Texture(elem));
            }
            let qsrc = smithay::utils::Rectangle::<f64, smithay::utils::Logical>::new(
                (0.0, 0.0).into(),
                (((out_size.w / 4).max(1)) as f64, ((out_size.h / 4).max(1)) as f64).into(),
            );
            let elem = smithay::backend::renderer::element::texture::TextureRenderElement::from_static_texture(
                smithay::backend::renderer::element::Id::new(), ctx.clone(), (0.0, 0.0),
                blurred.clone(), 1, Transform::Normal, Some(1.0), Some(qsrc), Some(out_size), None, Kind::Unspecified,
            );
            lelems.push(OutputRenderElements::Texture(elem));
        }
        let res = surface.compositor.render_frame(renderer, &lelems, Color32F::new(0.0, 0.0, 0.0, 1.0), FrameFlags::DEFAULT)
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
        // Screenshots while locked capture the locked frame only.
        if let Some(path) = state.screenshot.take() {
            let bg = Color32F::new(0.0, 0.0, 0.0, 1.0);
            match save_screenshot(renderer, &lelems, cursor_end, bg, out_size, scale, &path) {
                Ok(())  => tracing::info!(%path, "screenshot saved (locked)"),
                Err(e) => tracing::warn!(?e, %path, "screenshot failed"),
            }
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

    // ── unlock: melt the blur away over the live desktop ────────────────────
    if let Some((_, blurred, _)) = surface.lock_backdrop.take() {
        surface.lock_fade = Some((blurred, std::time::Instant::now()));
    }
    if let Some((blurred, t0)) = &surface.lock_fade {
        const FADE_MS: f32 = 450.0;
        let t = (t0.elapsed().as_secs_f32() * 1000.0 / FADE_MS).min(1.0);
        if t >= 1.0 {
            surface.lock_fade = None;
        } else {
            state.pending_redraw = true;
            let out_size = state.space.output_geometry(&surface.output)
                .map(|g| g.size)
                .unwrap_or_else(|| (1, 1).into());
            let ctx = smithay::backend::renderer::Renderer::context_id(renderer);
            let qsrc = smithay::utils::Rectangle::<f64, smithay::utils::Logical>::new(
                (0.0, 0.0).into(),
                (((out_size.w / 4).max(1)) as f64, ((out_size.h / 4).max(1)) as f64).into(),
            );
            let elem = smithay::backend::renderer::element::texture::TextureRenderElement::from_static_texture(
                smithay::backend::renderer::element::Id::new(), ctx.clone(), (0.0, 0.0),
                blurred.clone(), 1, Transform::Normal, Some((1.0 - t).powi(2)), Some(qsrc), Some(out_size), None, Kind::Unspecified,
            );
            elements.insert(after_cursor, OutputRenderElements::Texture(elem));
        }
    }

    // ── frosted glass ────────────────────────────────────────────────────────
    // Only runs while something wants it (the menu is open). The desktop
    // part of the element stack (everything under the menu) is re-rendered
    // into a 1/4-size offscreen texture, gaussian-blurred in four separable
    // passes, and a rounded crop of the result is slid in directly beneath
    // each requesting surface. The 4x downscale does half the softening and
    // keeps the passes cheap enough for virgl.
    if !blur_rects.is_empty() || !blur_windows.is_empty() {
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

            // Pass 0: the backdrop (wallpaper + anything below blur_mark stays
            // out — that's the menu/cursor) into texa, downscaled, back-to-front.
            // Window *content and borders* are skipped: the frost crop sits
            // BEHIND its window, so including the window would blur the window's
            // own pixels into its backdrop (the terminal text looked blurry).
            // What's left is the wallpaper — exactly "what's behind" for tiled
            // windows, which don't overlap.
            let scene = (|| -> std::result::Result<(), smithay::backend::renderer::gles::GlesError> {
                let mut fb = renderer.bind(texa)?;
                let mut frame = renderer.render(&mut fb, qsize, Transform::Normal)?;
                frame.clear(theme_clear, &[full])?;
                for elem in elements[blur_mark..].iter().rev() {
                    if matches!(elem,
                        OutputRenderElements::Window(_) | OutputRenderElements::Pixel(_)) {
                        continue;
                    }
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
                // Frosted backdrops behind translucent windows. Insert in
                // descending index order so each splice doesn't shift the
                // not-yet-placed (lower) indices.
                let mut bw = blur_windows.clone();
                bw.sort_by(|a, b| b.0.cmp(&a.0));
                for (idx, r, rad) in bw {
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
                    // Same corner radius as the window so the frost rounds to the
                    // exact same curve and never pokes past it.
                    let coff = [src.loc.x as f32 / qw as f32, src.loc.y as f32 / qh as f32];
                    let cscale = [src.size.w as f32 / qw as f32, src.size.h as f32 / qh as f32];
                    let patch = crate::render::BlurElement::new(
                        inner, frost_prog.clone(), rad, FROST_LIGHTEN, coff, cscale);
                    let at = idx.min(elements.len());
                    elements.insert(at, OutputRenderElements::Blur(patch));
                }
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
                    let coff = [src.loc.x as f32 / qw as f32, src.loc.y as f32 / qh as f32];
                    let cscale = [src.size.w as f32 / qw as f32, src.size.h as f32 / qh as f32];
                    let patch = crate::render::BlurElement::new(
                        inner, frost_prog.clone(), 16.0, FROST_LIGHTEN, coff, cscale);
                    elements.insert(blur_mark + i, OutputRenderElements::Blur(patch));
                }
            }
        }
    }

    // ── screenshot (IPC) ─────────────────────────────────────────────────────
    // Re-render the element stack (sans cursor) into an offscreen texture,
    // read it back, write PNG. One-shot; failures only log.
    if let Some(path) = state.screenshot.take() {
        let out_size = state.space.output_geometry(&surface.output)
            .map(|g| g.size)
            .unwrap_or_else(|| (1, 1).into());
        let bg = Color32F::new(theme.background[0], theme.background[1], theme.background[2], 1.0);
        match save_screenshot(renderer, &elements, after_cursor, bg, out_size, scale, &path) {
            Ok(())  => tracing::info!(%path, "screenshot saved"),
            Err(e) => tracing::warn!(?e, %path, "screenshot failed"),
        }
    }

    // ── wlr-screencopy (wf-recorder / grim / OBS / portals) ──────────────────
    // Same offscreen render + read-back as the screenshot path, but the bytes
    // go into each client's shm buffer instead of a PNG.
    if !state.pending_screencopy.is_empty() {
        // Capture buffer must be the PHYSICAL framebuffer size (mode), not the
        // logical output size — elements are drawn through geometry(scale), so
        // sizing the buffer to the logical size clipped everything to the
        // top-left under HiDPI. Using the mode makes the capture match scanout.
        let out_size = surface.output.current_mode()
            .map(|m| smithay::utils::Size::<i32, smithay::utils::Logical>::from((m.size.w, m.size.h)))
            .unwrap_or_else(|| (1, 1).into());
        let bg = Color32F::new(theme.background[0], theme.background[1], theme.background[2], 1.0);
        let output = surface.output.clone();
        fulfill_screencopy(renderer, &elements, after_cursor, bg, out_size, scale,
                           &output, &mut state.pending_screencopy);
    }

    // Keep the loop hot while animations are in flight.
    if anims_active {
        state.pending_redraw = true;
    }

    // Session-start fade-in: on this output's first frames, ease the whole
    // desktop up from black so it doesn't all snap in at once. Inserted last
    // (topmost, above the cursor) and AFTER screencopy so captures aren't
    // darkened. Set the clock lazily on the first rendered frame.
    let mut fading = false;
    {
        const FADE_IN_MS: f32 = 500.0;
        let fade_t = *surface.start_fade.get_or_insert(now);
        let fp = fade_t.elapsed().as_secs_f32() * 1000.0 / FADE_IN_MS;
        if fp < 1.0 {
            fading = true;
            let a = (1.0 - ease_out(fp)).clamp(0.0, 1.0);
            let phys = surface.output.current_mode().map(|m| m.size).unwrap_or((1, 1).into());
            elements.insert(0, OutputRenderElements::Solid(
                smithay::backend::renderer::element::solid::SolidColorRenderElement::new(
                    smithay::backend::renderer::element::Id::new(),
                    smithay::utils::Rectangle::from_size(phys),
                    0usize, // commit counter — fresh element each fade frame
                    Color32F::new(0.0, 0.0, 0.0, a),
                    Kind::Unspecified,
                ),
            ));
            state.pending_redraw = true;
        }
    }

    // Theme background (visible only where the wallpaper doesn't cover).
    let clear = Color32F::new(
        theme.background[0], theme.background[1], theme.background[2], 1.0,
    );
    // Wallpaper reveal + session fade composite a custom shader / alpha disc over
    // the wallpaper. The default frame flags let the DrmCompositor promote the
    // fullscreen opaque wallpaper straight to a hardware scanout plane — which
    // BYPASSES that GL compositing, so on NVIDIA the reveal froze and "popped"
    // to the final image while the GL render (and screenshots) were correct.
    // Force full GL composite (no plane scanout) for the duration of either.
    let transitioning = fading || surface.old_wallpaper.is_some();
    let frame_flags = if transitioning { FrameFlags::empty() } else { FrameFlags::DEFAULT };
    let res = surface.compositor.render_frame(renderer, &elements, clear, frame_flags)
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

fn on_libinput_event(event: InputEvent<LibinputInputBackend>, app: &mut State) {
    // Any real input resets the idle clock (device add/remove isn't activity)
    // and wakes the displays if they were powered off.
    if !matches!(event, InputEvent::DeviceAdded { .. } | InputEvent::DeviceRemoved { .. }) {
        app.last_activity = std::time::Instant::now();
        app.auto_lock_fired = false;
        // Re-arm the screensaver for the next idle stretch. Without this, a run
        // that self-skipped (on battery / no video set) leaves screensaver_fired
        // stuck true and the screensaver never fires again.
        app.screensaver_fired = false;
        if app.screen_off {
            app.screen_off = false;
            wake_outputs(app);
        }
        // A running screensaver starts fading out on the first real input.
        // We must NOT swallow keyboard events: the compositor tracks key
        // repeat by press/release, and eating a release leaves the key "stuck
        // down" auto-repeating into the focused app. So swallow only pointer /
        // touch (no repeat) — a dismissing click/tap shouldn't also land on
        // whatever's underneath; a dismissing keystroke harmlessly does.
        if app.begin_screensaver_dismiss()
            && !matches!(event, InputEvent::Keyboard { .. })
        {
            return;
        }
    }
    let state = &mut *app;
    match event {
        InputEvent::DeviceAdded { mut device } => {
            let cfg = &state.config;
            // Touchpad (anything with tap support): tap-to-click, tap-and-drag,
            // disable-while-typing — each overridable from the `input` block.
            if device.config_tap_finger_count() > 0 {
                let _ = device.config_tap_set_enabled(cfg.tap_to_click.unwrap_or(true));
                let _ = device.config_tap_set_drag_enabled(true);
                let _ = device.config_dwt_set_enabled(cfg.disable_while_typing.unwrap_or(true));
            }
            // Natural ("reverse") scrolling defaults on everywhere (touchpad AND
            // mouse wheel) so content tracks the fingers/wheel like macOS;
            // `natural-scroll #false` opts out. No-op where unsupported.
            let _ = device.config_scroll_set_natural_scroll_enabled(cfg.natural_scroll.unwrap_or(true));
            if let Some(a) = cfg.accel_speed {
                let _ = device.config_accel_set_speed(a.clamp(-1.0, 1.0));
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
            maybe_focus_follows_mouse(state);
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
            maybe_focus_follows_mouse(state);
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
                // Super+LeftDrag = move, Super+RightDrag = resize (floating free,
                // tiled trades split ratios). Shared with touch emulation.
                if logo && (code == BTN_LEFT || code == BTN_RIGHT)
                    && state.try_begin_super_drag(code)
                {
                    return;   // the client never sees this press
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

        // ── touchscreen (single-finger pointer emulation) ───────────────────
        // vendiOS targets laptops with a touchscreen, so touch acts like the
        // mouse instead of delivering native wl_touch: tap = click, drag =
        // left-drag, still hold = right click, Super-held = window move. The
        // tap/drag/long-press machine lives in State (touch_down/motion/up/tick).
        InputEvent::TouchDown { event } => {
            if state.vlock { return; }
            use smithay::backend::input::TouchEvent as _;
            let Some(output) = state.space.outputs().next().cloned() else { return };
            let Some(geo) = state.space.output_geometry(&output) else { return };
            let pos = event.position_transformed(geo.size) + geo.loc.to_f64();
            let super_held = state.seat.get_keyboard()
                .map(|k| k.modifier_state().logo).unwrap_or(false);
            state.touch_down(event.slot(), pos, InputEventTrait::time_msec(&event), super_held);
        }
        InputEvent::TouchMotion { event } => {
            if state.vlock { return; }
            use smithay::backend::input::TouchEvent as _;
            let Some(output) = state.space.outputs().next().cloned() else { return };
            let Some(geo) = state.space.output_geometry(&output) else { return };
            let pos = event.position_transformed(geo.size) + geo.loc.to_f64();
            state.touch_motion(event.slot(), pos, InputEventTrait::time_msec(&event));
        }
        InputEvent::TouchUp { event } => {
            if state.vlock { return; }
            use smithay::backend::input::TouchEvent as _;
            state.touch_up(event.slot(), InputEventTrait::time_msec(&event));
        }
        InputEvent::TouchFrame { .. } => {}
        InputEvent::TouchCancel { .. } => {
            if state.touch.is_some() || state.touch_gesture.is_some() {
                state.touch_reset();
                state.pending_redraw = true;
            }
        }

        _ => {}
    }
}

fn on_udev_event(event: UdevEvent, app: &mut State) {
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
                if app.udev.as_mut().unwrap().drm_devices.contains_key(&node) {
                    rescan_connectors(app, node);
                }
            }
        }
        UdevEvent::Removed { device_id }       => tracing::info!(?device_id,        "udev: device removed"),
    }
}
