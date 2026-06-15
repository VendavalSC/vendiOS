// Premium render pieces: rounded window corners + SDF rounded borders.
//
// Corners: client textures are drawn through a custom fragment shader that
// masks pixels outside a rounded-rect SDF (antialiased, 1px feather). The
// shader is applied per element via `GlesFrame::override_default_tex_program`
// inside a wrapper element (`RoundedElement`) — the same pattern niri uses.
//
// Borders: a `PixelShaderElement` draws an antialiased rounded ring directly
// on the GPU, replacing the old square `SolidColorRenderElement` frames.

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                Element, Id, Kind, RenderElement, UnderlyingStorage,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                surface::WaylandSurfaceRenderElement,
                texture::TextureRenderElement,
            },
            gles::{GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform},
            utils::{CommitCounter, DamageSet, OpaqueRegions},
        },
    },
    utils::{Buffer as BufferCoords, Physical, Point, Rectangle, Scale, Transform},
};

/// Corner radius of window content, logical px.
pub const RADIUS: f32 = 12.0;

/// Rounded-corner texture shader. Masks the sampled color against a
/// rounded-box SDF over the element's own quad.
pub const ROUNDED_TEX_FRAG: &str = r#"#version 100
//_DEFINES

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision mediump float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

uniform vec2 size;
uniform float radius;

float rounded_box(vec2 p, vec2 half_size, float r) {
    vec2 q = abs(p) - half_size + r;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}

void main() {
    vec4 color = texture2D(tex, v_coords);
#if defined(NO_ALPHA_MULTIPLIER)
    color.a = 1.0;
#endif
    vec2 p = (v_coords - vec2(0.5)) * size;
    float d = rounded_box(p, size * 0.5, radius);
    float mask = 1.0 - smoothstep(-1.0, 1.0, d);
    color *= alpha * mask;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.3, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}
"#;

/// Frosted-glass material (texture shader): the rounded-corner crop of the
/// blurred desktop, lifted toward a light, faintly desaturated tone. A plain
/// gaussian blur of the (dark) wallpaper reads as a dark smear behind a
/// translucent window; `lighten` blends it toward white so it looks like real
/// frosted glass instead. `lighten` 0 == identical to the rounded shader.
pub const FROST_TEX_FRAG: &str = r#"#version 100
//_DEFINES

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision mediump float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

uniform vec2 size;
uniform float radius;
uniform float lighten;

float rounded_box(vec2 p, vec2 half_size, float r) {
    vec2 q = abs(p) - half_size + r;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}

void main() {
    vec4 color = texture2D(tex, v_coords);
#if defined(NO_ALPHA_MULTIPLIER)
    color.a = 1.0;
#endif
    // Material lift: gently desaturate, then blend toward white so the frost
    // brightens the dark desktop rather than darkening the window over it.
    vec3 rgb = color.rgb;
    float luma = dot(rgb, vec3(0.299, 0.587, 0.114));
    rgb = mix(rgb, vec3(luma), 0.12);
    rgb = mix(rgb, vec3(1.0), lighten);
    color.rgb = rgb;
    vec2 p = (v_coords - vec2(0.5)) * size;
    float d = rounded_box(p, size * 0.5, radius);
    float mask = 1.0 - smoothstep(-1.0, 1.0, d);
    color *= alpha * mask;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.3, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}
"#;

/// Separable gaussian blur pass (texture shader). `dir` carries the sample
/// step in UV space — (r/w, 0) for the horizontal pass, (0, r/h) for the
/// vertical one. Run over a downscaled copy of the desktop; the downscale
/// itself contributes most of the softness.
pub const BLUR_FRAG: &str = r#"#version 100
//_DEFINES

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision mediump float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

uniform vec2 dir;

void main() {
    vec4 sum = texture2D(tex, v_coords) * 0.2270270270;
    sum += texture2D(tex, v_coords + dir * 1.3846153846) * 0.3162162162;
    sum += texture2D(tex, v_coords - dir * 1.3846153846) * 0.3162162162;
    sum += texture2D(tex, v_coords + dir * 3.2307692308) * 0.0702702703;
    sum += texture2D(tex, v_coords - dir * 3.2307692308) * 0.0702702703;
    gl_FragColor = sum * alpha;
}
"#;

/// Rounded border ring (pixel shader): premultiplied color masked to the
/// band between the outer rounded rect and the same rect inset by
/// `thickness`.
// NOTE: unlike texture shaders, smithay prepends `#version 100` (and the
// DEBUG_FLAGS define in the debug variant) to pixel shader sources itself —
// no version line or //_DEFINES marker here.
pub const BORDER_FRAG: &str = r#"precision mediump float;
uniform float alpha;
uniform vec2 size;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

uniform vec4 color;
uniform float radius;
uniform float thickness;

float rounded_box(vec2 p, vec2 half_size, float r) {
    vec2 q = abs(p) - half_size + r;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}

void main() {
    vec2 p = (v_coords - vec2(0.5)) * size;
    float d = rounded_box(p, size * 0.5, radius);
    float outer = 1.0 - smoothstep(-1.0, 0.5, d);
    float inner = smoothstep(-thickness - 0.5, -thickness + 1.0, d);
    float a = color.a * outer * inner * alpha;
    gl_FragColor = vec4(color.rgb * a, a);
}
"#;

/// Circular reveal (texture shader): the sampled color is kept inside a
/// growing circle and discarded outside — the new wallpaper "blooms" over
/// the old one from the point where the switch happened.
pub const REVEAL_FRAG: &str = r#"#version 100
//_DEFINES

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision mediump float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

uniform vec2 size;
uniform vec2 center;
uniform float reveal;

void main() {
    vec4 color = texture2D(tex, v_coords);
#if defined(NO_ALPHA_MULTIPLIER)
    color.a = 1.0;
#endif
    float d = length(v_coords * size - center) - reveal;
    // ~2.5px feathered rim so the edge reads soft while it sweeps.
    float mask = 1.0 - smoothstep(-2.5, 2.5, d);
    color *= alpha * mask;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.3, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}
"#;

// ── wallpaper ─────────────────────────────────────────────────────────────────

/// Build the output-sized wallpaper buffer. A user image (from the theme's
/// `wallpaper` path, cover-scaled) wins; otherwise a procedural obsidian
/// gradient derived from the theme: vertical fade of the background color, a
/// soft accent glow up top, and a faint shard watermark in the lower right.
pub fn wallpaper_buffer(
    w: i32,
    h: i32,
    path: Option<&str>,
    background: [f32; 4],
    accent: [f32; 4],
) -> MemoryRenderBuffer {
    if let Some(path) = path {
        match load_wallpaper(path, w, h) {
            Ok(buf) => return buf,
            Err(e) => tracing::warn!(?e, %path, "wallpaper load failed; using gradient"),
        }
    }
    procedural_wallpaper(w, h, background, accent)
}

fn load_wallpaper(path: &str, w: i32, h: i32) -> anyhow::Result<MemoryRenderBuffer> {
    let img = image::open(path)?;
    // Cover-scale: fill the output, cropping overflow.
    let img = img.resize_to_fill(w as u32, h as u32, image::imageops::FilterType::Triangle);
    let rgba = img.to_rgba8();
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for px in rgba.pixels() {
        // Argb8888 little-endian byte order: B G R A. Wallpaper is opaque.
        data.extend_from_slice(&[px[2], px[1], px[0], 255]);
    }
    Ok(MemoryRenderBuffer::from_slice(
        &data, Fourcc::Argb8888, (w, h), 1, Transform::Normal, None,
    ))
}

fn procedural_wallpaper(w: i32, h: i32, background: [f32; 4], accent: [f32; 4]) -> MemoryRenderBuffer {
    let (wf, hf) = (w as f32, h as f32);
    let mut data = vec![0u8; (w * h * 4) as usize];

    // Theme colors in 0-255 space. The fade bottom is the background pulled
    // ~42% toward black — the same base→crust feel Mocha had.
    let top = (background[0] * 255.0, background[1] * 255.0, background[2] * 255.0);
    let bot = (top.0 * 0.58, top.1 * 0.58, top.2 * 0.58);
    let (mr, mg, mb) = (accent[0] * 255.0, accent[1] * 255.0, accent[2] * 255.0);

    // Shard watermark geometry (mirrors the vendibar logo), anchored in the
    // lower-right quadrant at ~55% of screen height.
    let s = hf * 0.55;
    let (ox, oy) = (wf * 0.80 - s * 0.5, hf * 0.62 - s * 0.5);
    let t = (ox + s * 0.42, oy + s * 0.04);
    let r = (ox + s * 0.92, oy + s * 0.30);
    let b = (ox + s * 0.60, oy + s * 0.97);
    let l = (ox + s * 0.10, oy + s * 0.46);
    fn in_tri(p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
        let sign = |p1: (f32, f32), p2: (f32, f32), p3: (f32, f32)| {
            (p1.0 - p3.0) * (p2.1 - p3.1) - (p2.0 - p3.0) * (p1.1 - p3.1)
        };
        let (d1, d2, d3) = (sign(p, a, b), sign(p, b, c), sign(p, c, a));
        let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
        let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
        !(has_neg && has_pos)
    }

    for y in 0..h {
        let ty = y as f32 / hf;
        // Vertical fade: theme background → its darkened crust.
        let bg = (
            top.0 + (bot.0 - top.0) * ty,
            top.1 + (bot.1 - top.1) * ty,
            top.2 + (bot.2 - top.2) * ty,
        );
        for x in 0..w {
            let tx = x as f32 / wf;
            let p = (x as f32, y as f32);

            // Soft mauve glow near the top center.
            let (gx, gy) = (tx - 0.5, ty - 0.10);
            let glow = (1.0 - (gx * gx * 3.2 + gy * gy * 5.0).sqrt()).clamp(0.0, 1.0);
            let glow = glow * glow * 0.10;

            // Shard watermark: two faces at slightly different strengths.
            let shard = if in_tri(p, t, b, l) { 0.050 }
                else if in_tri(p, t, r, b)    { 0.085 }
                else { 0.0 };

            let mix = glow + shard;
            let rr = (bg.0 + (mr - bg.0) * mix).round() as u8;
            let gg = (bg.1 + (mg - bg.1) * mix).round() as u8;
            let bb = (bg.2 + (mb - bg.2) * mix).round() as u8;

            let i = ((y * w + x) * 4) as usize;
            data[i]     = bb;   // B
            data[i + 1] = gg;   // G
            data[i + 2] = rr;   // R
            data[i + 3] = 255;  // A
        }
    }

    MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (w, h), 1, Transform::Normal, None)
}

/// Wraps a window's `WaylandSurfaceRenderElement` and draws it through the
/// rounded-corner shader. Everything else delegates to the inner element.
pub struct RoundedElement {
    inner:   WaylandSurfaceRenderElement<GlesRenderer>,
    program: GlesTexProgram,
    radius:  f32,
}

impl RoundedElement {
    pub fn new(
        inner: WaylandSurfaceRenderElement<GlesRenderer>,
        program: GlesTexProgram,
        radius: f32,
    ) -> Self {
        Self { inner, program, radius }
    }
}

impl Element for RoundedElement {
    fn id(&self) -> &Id { self.inner.id() }
    fn current_commit(&self) -> CommitCounter { self.inner.current_commit() }
    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> { self.inner.location(scale) }
    fn src(&self) -> Rectangle<f64, BufferCoords> { self.inner.src() }
    fn transform(&self) -> Transform { self.inner.transform() }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> { self.inner.geometry(scale) }
    fn damage_since(&self, scale: Scale<f64>, commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }
    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        if self.radius <= 0.0 {
            return self.inner.opaque_regions(scale);
        }
        // The corners are punched transparent by the shader, so the element
        // can't advertise opaque regions — keep it simple and claim none.
        OpaqueRegions::default()
    }
    fn alpha(&self) -> f32 { self.inner.alpha() }
    fn kind(&self) -> Kind { self.inner.kind() }
}

impl RenderElement<GlesRenderer> for RoundedElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&smithay::utils::user_data::UserDataMap>,
    ) -> Result<(), GlesError> {
        if self.radius <= 0.0 {
            return RenderElement::<GlesRenderer>::draw(
                &self.inner, frame, src, dst, damage, opaque_regions, cache,
            );
        }
        frame.override_default_tex_program(
            self.program.clone(),
            vec![
                Uniform::new("size", (dst.size.w as f32, dst.size.h as f32)),
                Uniform::new("radius", self.radius),
            ],
        );
        let res = RenderElement::<GlesRenderer>::draw(
            &self.inner, frame, src, dst, damage, opaque_regions, cache,
        );
        frame.clear_tex_program_override();
        res
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        // Refuse direct scanout: the corners only exist when the shader runs.
        None
    }
}

/// Wraps the incoming wallpaper and draws it through the circular-reveal
/// shader: only the disc of radius `reveal` around `center` (element-local
/// px) survives. Drawn over the outgoing wallpaper, radius eased to the far
/// corner — the swww-style bloom.
pub struct RevealElement {
    inner:   MemoryRenderBufferRenderElement<GlesRenderer>,
    program: GlesTexProgram,
    center:  (f32, f32),
    reveal:  f32,
}

impl RevealElement {
    pub fn new(
        inner: MemoryRenderBufferRenderElement<GlesRenderer>,
        program: GlesTexProgram,
        center: (f32, f32),
        reveal: f32,
    ) -> Self {
        Self { inner, program, center, reveal }
    }
}

impl Element for RevealElement {
    fn id(&self) -> &Id { self.inner.id() }
    fn current_commit(&self) -> CommitCounter { self.inner.current_commit() }
    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> { self.inner.location(scale) }
    fn src(&self) -> Rectangle<f64, BufferCoords> { self.inner.src() }
    fn transform(&self) -> Transform { self.inner.transform() }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> { self.inner.geometry(scale) }
    fn damage_since(&self, scale: Scale<f64>, _commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
        // The mask moves every frame of the transition — damage everything.
        DamageSet::from_slice(&[Rectangle::from_size(self.geometry(scale).size)])
    }
    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        // Transparent outside the disc by construction.
        OpaqueRegions::default()
    }
    fn alpha(&self) -> f32 { self.inner.alpha() }
    fn kind(&self) -> Kind { self.inner.kind() }
}

impl RenderElement<GlesRenderer> for RevealElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&smithay::utils::user_data::UserDataMap>,
    ) -> Result<(), GlesError> {
        frame.override_default_tex_program(
            self.program.clone(),
            vec![
                Uniform::new("size", (dst.size.w as f32, dst.size.h as f32)),
                Uniform::new("center", self.center),
                Uniform::new("reveal", self.reveal),
            ],
        );
        let res = RenderElement::<GlesRenderer>::draw(
            &self.inner, frame, src, dst, damage, opaque_regions, cache,
        );
        frame.clear_tex_program_override();
        res
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        // The disc only exists when the shader runs — never scan out.
        None
    }
}

/// Frosted-glass patch: a crop of the blurred desktop texture, drawn through
/// the rounded-corner shader so it hugs the card it sits behind (vendi-menu).
pub struct BlurElement {
    inner:   TextureRenderElement<GlesTexture>,
    program: GlesTexProgram,
    radius:  f32,
    lighten: f32,
}

impl BlurElement {
    pub fn new(
        inner: TextureRenderElement<GlesTexture>,
        program: GlesTexProgram,
        radius: f32,
        lighten: f32,
    ) -> Self {
        Self { inner, program, radius, lighten }
    }
}

impl Element for BlurElement {
    fn id(&self) -> &Id { self.inner.id() }
    fn current_commit(&self) -> CommitCounter { self.inner.current_commit() }
    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> { self.inner.location(scale) }
    fn src(&self) -> Rectangle<f64, BufferCoords> { self.inner.src() }
    fn transform(&self) -> Transform { self.inner.transform() }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> { self.inner.geometry(scale) }
    fn damage_since(&self, scale: Scale<f64>, commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }
    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }
    fn alpha(&self) -> f32 { self.inner.alpha() }
    fn kind(&self) -> Kind { self.inner.kind() }
}

impl RenderElement<GlesRenderer> for BlurElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&smithay::utils::user_data::UserDataMap>,
    ) -> Result<(), GlesError> {
        frame.override_default_tex_program(
            self.program.clone(),
            vec![
                Uniform::new("size", (dst.size.w as f32, dst.size.h as f32)),
                Uniform::new("radius", self.radius),
                Uniform::new("lighten", self.lighten),
            ],
        );
        let res = RenderElement::<GlesRenderer>::draw(
            &self.inner, frame, src, dst, damage, opaque_regions, cache,
        );
        frame.clear_tex_program_override();
        res
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}
