// Cursor loading + render-element wrapping.
//
// Reads XCURSOR_THEME / XCURSOR_SIZE (or "default" / 24), parses the matching
// .xcursor file via the `xcursor` crate, and bakes the chosen frame into a
// `MemoryRenderBuffer` the renderer can blit. Falls back to an embedded 64×64
// arrow if no theme is found — guarantees the pointer is always visible even
// on a minimal install with no cursor packages.

use std::io::Read;

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::element::memory::MemoryRenderBuffer,
    },
    utils::Transform,
};
use smithay::input::pointer::CursorIcon;
use xcursor::{CursorTheme, parser::{Image, parse_xcursor}};

/// Embedded fallback cursor: 64×64 RGBA, an arrow. Lifted from smithay/anvil
/// (MIT/Apache-2.0) so we don't depend on a cursor theme being installed.
static FALLBACK_CURSOR: &[u8] = include_bytes!("cursor.rgba");

pub struct Cursor {
    pub buffer:  MemoryRenderBuffer,
    /// Hotspot relative to the cursor image's top-left corner. The pointer's
    /// logical position should land on this pixel.
    pub hotspot: (i32, i32),
}

impl Cursor {
    /// The plain arrow ("default" shape), or the embedded fallback.
    pub fn load() -> Self {
        Self::load_named(&["default"])
    }

    /// Load the themed cursor for a client-requested shape (cursor-shape-v1 /
    /// wl_pointer.set_cursor named icons): a hand for links, an I-beam for text,
    /// a wait spinner, resize arrows, etc. Tries the icon's canonical xcursor
    /// name then its alternates (themes name these inconsistently), falling back
    /// to the plain arrow so the pointer is never invisible.
    pub fn load_icon(icon: CursorIcon) -> Self {
        let mut names: Vec<&str> = Vec::with_capacity(1 + icon.alt_names().len());
        names.push(icon.name());
        names.extend_from_slice(icon.alt_names());
        Self::load_named(&names)
    }

    /// Load the first of `names` that resolves in the configured theme, else the
    /// embedded arrow.
    pub fn load_named(names: &[&str]) -> Self {
        let theme_name = std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".into());
        let size: u32 = std::env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(24);

        let image = names.iter().find_map(|n| load_theme_icon(&theme_name, n, size));

        match image {
            Some(img) => Self::from_image(&img),
            None => {
                tracing::debug!(theme = %theme_name, ?names, "cursor shape not in theme; using arrow fallback");
                // The embedded fallback only knows the arrow; reuse it for any
                // unresolved shape rather than showing nothing.
                Self::from_rgba(64, 64, 1, 1, FALLBACK_CURSOR)
            }
        }
    }

    fn from_image(img: &Image) -> Self {
        Self::from_rgba(
            img.width as i32, img.height as i32,
            img.xhot as i32, img.yhot as i32,
            &img.pixels_rgba,
        )
    }

    fn from_rgba(width: i32, height: i32, xhot: i32, yhot: i32, pixels: &[u8]) -> Self {
        // XCursor delivers raw RGBA bytes (R, G, B, A in memory). On a
        // little-endian box that's an Abgr8888 packed pixel.
        let buffer = MemoryRenderBuffer::from_slice(
            pixels, Fourcc::Abgr8888, (width, height), 1, Transform::Normal, None,
        );
        Self { buffer, hotspot: (xhot, yhot) }
    }
}

/// Best-effort load of `<theme>/cursors/<name>`, return None on any failure
/// (theme missing, shape absent, file unreadable, parse fail, no images).
fn load_theme_icon(theme_name: &str, name: &str, requested_size: u32) -> Option<Image> {
    let theme = CursorTheme::load(theme_name);
    let path  = theme.load_icon(name)?;
    let mut file = std::fs::File::open(path).ok()?;
    let mut data = Vec::new();
    file.read_to_end(&mut data).ok()?;
    let images = parse_xcursor(&data)?;
    images.into_iter()
        .min_by_key(|i| (i.size as i32 - requested_size as i32).abs())
}
