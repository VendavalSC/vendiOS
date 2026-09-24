// Compositor-drawn text — tab titles and shelf labels.
//
// vendiwm otherwise never draws text (everything textual lives in the bar),
// but window chrome that belongs to the compositor — the tab strip of a
// grouped tile, the labels under shelf cards — has to be drawn here. fontdue
// rasterizes glyph coverage; we composite it into a premultiplied ARGB buffer
// in the requested colour. The backend wraps that in a MemoryRenderBuffer and
// caches it, so a title is rasterized once, not every frame.

use std::sync::OnceLock;

use fontdue::{Font, FontSettings};

/// The UI font: the same monospace the bar uses (JetBrainsMono Nerd Font on
/// vendiOS), resolved through fontconfig once, with plain-path fallbacks.
pub fn font() -> Option<&'static Font> {
    static FONT: OnceLock<Option<Font>> = OnceLock::new();
    FONT.get_or_init(|| {
        let mut candidates: Vec<String> = Vec::new();
        for family in ["JetBrainsMonoNL Nerd Font", "JetBrainsMono Nerd Font", "monospace", "sans-serif"] {
            if let Ok(out) = std::process::Command::new("fc-match")
                .args(["-f", "%{file}", family])
                .output()
            {
                let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !p.is_empty() { candidates.push(p); }
            }
        }
        candidates.extend([
            "/usr/share/fonts/TTF/JetBrainsMonoNLNerdFont-Regular.ttf",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Regular.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        ].map(String::from));
        for path in candidates {
            // fontdue reads TrueType/OpenType; skip collections and bitmaps.
            if !(path.ends_with(".ttf") || path.ends_with(".otf")) { continue; }
            let Ok(bytes) = std::fs::read(&path) else { continue };
            match Font::from_bytes(bytes, FontSettings::default()) {
                Ok(f) => {
                    tracing::info!(%path, "chrome font loaded");
                    return Some(f);
                }
                Err(e) => tracing::warn!(%path, e, "chrome font rejected"),
            }
        }
        tracing::warn!("no usable font for compositor text — tab titles hidden");
        None
    }).as_ref()
}

/// Advance width of `text` at `px` (physical pixels).
pub fn measure(font: &Font, text: &str, px: f32) -> f32 {
    text.chars().map(|c| font.metrics(c, px).advance_width).sum()
}

/// `text` shortened with a trailing ellipsis so it fits `max_w` (physical px).
pub fn elide(font: &Font, text: &str, px: f32, max_w: f32) -> String {
    if measure(font, text, px) <= max_w { return text.to_string(); }
    let ell = font.metrics('…', px).advance_width;
    let mut out = String::new();
    let mut w = 0.0;
    for c in text.chars() {
        let a = font.metrics(c, px).advance_width;
        if w + a + ell > max_w { break; }
        w += a;
        out.push(c);
    }
    let trimmed = out.trim_end().to_string();
    if trimmed.is_empty() { String::new() } else { trimmed + "…" }
}

/// A rasterized line: premultiplied ARGB8888 (little-endian BGRA bytes).
pub struct Raster {
    pub data: Vec<u8>,
    pub w: i32,
    pub h: i32,
}

/// Rasterize one line of `text` at `px` physical pixels in `color` (straight
/// RGBA 0..1), elided to `max_w` physical px. None for empty text / no font.
pub fn rasterize(text: &str, px: f32, color: [f32; 4], max_w: f32) -> Option<Raster> {
    let font = font()?;
    let text = elide(font, text, px, max_w.max(1.0));
    if text.is_empty() { return None; }
    let lm = font.horizontal_line_metrics(px)?;
    let w = measure(font, &text, px).ceil() as i32 + 2;
    let h = (lm.ascent - lm.descent).ceil() as i32 + 2;
    if w <= 0 || h <= 0 { return None; }
    let baseline = lm.ascent.round() as i32 + 1;
    let mut cov = vec![0u8; (w * h) as usize];
    let mut pen = 1.0f32;
    for c in text.chars() {
        let (m, bitmap) = font.rasterize(c, px);
        let gx = (pen + m.xmin as f32).round() as i32;
        let gy = baseline - m.ymin - m.height as i32;
        for row in 0..m.height as i32 {
            for col in 0..m.width as i32 {
                let (x, y) = (gx + col, gy + row);
                if x < 0 || y < 0 || x >= w || y >= h { continue; }
                let a = bitmap[(row * m.width as i32 + col) as usize];
                let i = (y * w + x) as usize;
                cov[i] = cov[i].max(a);
            }
        }
        pen += m.advance_width;
    }
    let mut data = vec![0u8; (w * h * 4) as usize];
    for (i, a) in cov.into_iter().enumerate() {
        if a == 0 { continue; }
        let alpha = a as f32 / 255.0 * color[3];
        let p = |c: f32| (c * alpha * 255.0).round().clamp(0.0, 255.0) as u8;
        data[i * 4]     = p(color[2]);
        data[i * 4 + 1] = p(color[1]);
        data[i * 4 + 2] = p(color[0]);
        data[i * 4 + 3] = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    Some(Raster { data, w, h })
}

/// Rasterized text kept as render buffers, keyed by everything that changes
/// the pixels. Entries unused for a few seconds are dropped by `sweep`.
#[derive(Default)]
pub struct TextCache {
    map: std::collections::HashMap<(String, u32, [u8; 4], u32),
        (smithay::backend::renderer::element::memory::MemoryRenderBuffer, (i32, i32), std::time::Instant)>,
}

impl TextCache {
    /// Buffer + physical size for `text` at `px` physical pixels. The colour's
    /// alpha is ignored — apply it on the render element, so fades don't
    /// rasterize a fresh buffer every frame.
    pub fn get(&mut self, text: &str, px: f32, color: [f32; 4], max_w: f32)
        -> Option<(&smithay::backend::renderer::element::memory::MemoryRenderBuffer, (i32, i32))>
    {
        let q = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
        let color = [color[0], color[1], color[2], 1.0];
        let key = (text.to_string(), (px * 10.0) as u32, [q(color[0]), q(color[1]), q(color[2]), 255],
                   max_w.max(0.0) as u32);
        let now = std::time::Instant::now();
        if !self.map.contains_key(&key) {
            let r = rasterize(text, px, color, max_w)?;
            let buf = smithay::backend::renderer::element::memory::MemoryRenderBuffer::from_slice(
                &r.data, smithay::backend::allocator::Fourcc::Argb8888, (r.w, r.h), 1,
                smithay::utils::Transform::Normal, None,
            );
            self.map.insert(key.clone(), (buf, (r.w, r.h), now));
        }
        let e = self.map.get_mut(&key)?;
        e.2 = now;
        Some((&e.0, e.1))
    }

    pub fn sweep(&mut self) {
        let now = std::time::Instant::now();
        self.map.retain(|_, (_, _, t)| now.duration_since(*t).as_secs() < 5);
    }
}
