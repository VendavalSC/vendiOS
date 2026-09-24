// Window chrome the compositor draws itself: the tab strip over a grouped
// tile and the stage shelf (the scratchpad, shown as live cards down the left
// edge, Stage Manager style).
//
// State builds a backend-agnostic scene of plain primitives each frame
// (`State::chrome_scene`); each backend turns those into its own render
// elements. Keeping it primitive-only means the nested winit backend can draw
// the same chrome as the real udev session.

use smithay::desktop::Window;
use smithay::utils::{Logical, Rectangle};

/// Height of a group's tab strip, logical px, and the gap between the strip
/// and the window it heads.
pub const TAB_H: i32 = 30;
pub const TAB_GAP: i32 = 6;
/// Width the stage shelf takes from the left of the tiling area.
pub const SHELF_W: i32 = 208;

#[derive(Clone)]
pub enum Item {
    /// Filled rounded rectangle.
    Rect { rect: Rectangle<f64, Logical>, radius: f32, color: [f32; 4] },
    /// Rounded outline, `thickness` drawn inward from `rect`'s edge.
    Ring { rect: Rectangle<f64, Logical>, radius: f32, thickness: f32, color: [f32; 4] },
    /// One line of text, vertically centered on `cy`, starting at `x` (or
    /// centered in [x, x+max_w] when `center`), elided to `max_w`.
    Text { x: f64, cy: f64, max_w: f64, text: String, px: f32, color: [f32; 4], center: bool },
    /// A window's live contents scaled into `rect` (shelf cards).
    Thumb { window: Window, rect: Rectangle<f64, Logical>, radius: f32, alpha: f32 },
}

/// Tabs of one grouped tile, as the renderer needs them.
#[derive(Clone)]
pub struct TabInfo {
    pub titles:  Vec<String>,
    pub active:  usize,
    /// Tab under the pointer (index), for the hover wash.
    pub hover:   Option<usize>,
    /// The tile currently holds keyboard focus (strip reads brighter).
    pub focused: bool,
}

/// Everything chrome for one frame.
#[derive(Default)]
pub struct Scene {
    /// Tab strips keyed by the window id of the tile's active member; drawn
    /// with that window (so they share its z-order and ride its animation).
    pub tabs:  std::collections::HashMap<u32, TabInfo>,
    /// Shelf cards + labels, global logical coords, below all windows.
    pub shelf: Vec<Item>,
    /// Something in the chrome is mid-animation — keep frames coming.
    pub animating: bool,
    /// Shelf windows to send frame callbacks to (live thumbnails).
    pub live: Vec<Window>,
}

/// Palette the chrome draws with, derived from the theme.
#[derive(Clone, Copy)]
pub struct Palette {
    pub accent:   [f32; 4],
    pub inactive: [f32; 4],
    pub bg:       [f32; 4],
    pub fg:       [f32; 4],
    pub dim:      [f32; 4],
}

impl Palette {
    pub fn from_theme(t: &crate::config::Theme) -> Self {
        let bg = t.background;
        let lum = 0.2126 * bg[0] + 0.7152 * bg[1] + 0.0722 * bg[2];
        let (fg, dim) = if lum < 0.5 {
            ([0.93, 0.94, 0.97, 1.0], [0.93, 0.94, 0.97, 0.55])
        } else {
            ([0.10, 0.10, 0.13, 1.0], [0.10, 0.10, 0.13, 0.55])
        };
        Self { accent: t.accent, inactive: t.inactive, bg, fg, dim }
    }
}

pub fn with_alpha(c: [f32; 4], a: f32) -> [f32; 4] { [c[0], c[1], c[2], c[3] * a] }

/// Tab slot rects inside a strip of width `w` (strip-local, y from 0).
pub fn tab_slots(w: f64, n: usize) -> Vec<Rectangle<f64, Logical>> {
    const PAD: f64 = 3.0;
    const SEP: f64 = 3.0;
    if n == 0 { return Vec::new(); }
    let inner = (w - PAD * 2.0 - SEP * (n as f64 - 1.0)).max(1.0);
    let tw = inner / n as f64;
    (0..n).map(|i| Rectangle::new(
        (PAD + i as f64 * (tw + SEP), PAD).into(),
        (tw, TAB_H as f64 - PAD * 2.0).into(),
    )).collect()
}

/// The strip for one grouped tile, laid out for `w` wide, strip-local coords
/// (0,0 = the strip's top-left). The renderer offsets it above the window's
/// animated rect, so the strip rides every morph with its window.
pub fn strip_items(info: &TabInfo, w: f64, radius: f32, pal: &Palette, alpha: f32) -> Vec<Item> {
    let mut out = Vec::new();
    let r = (radius * 0.75).clamp(6.0, 12.0);
    let body = Rectangle::new((0.0, 0.0).into(), (w, TAB_H as f64).into());
    out.push(Item::Rect { rect: body, radius: r, color: with_alpha(pal.bg, 0.78 * alpha) });
    out.push(Item::Ring {
        rect: body, radius: r, thickness: 1.0,
        color: with_alpha(if info.focused { pal.accent } else { pal.inactive }, 0.55 * alpha),
    });
    let slots = tab_slots(w, info.titles.len());
    for (i, (slot, title)) in slots.iter().zip(&info.titles).enumerate() {
        let active = i == info.active;
        let tr = (r - 3.0).max(4.0);
        if active {
            let a = if info.focused { 0.26 } else { 0.14 };
            out.push(Item::Rect { rect: *slot, radius: tr, color: with_alpha(pal.accent, a * alpha) });
        } else if info.hover == Some(i) {
            out.push(Item::Rect { rect: *slot, radius: tr, color: with_alpha(pal.fg, 0.07 * alpha) });
        }
        let color = if active {
            with_alpha(if info.focused { pal.accent } else { pal.fg }, alpha)
        } else {
            with_alpha(pal.dim, alpha)
        };
        out.push(Item::Text {
            x: slot.loc.x + 10.0, cy: slot.loc.y + slot.size.h / 2.0,
            max_w: slot.size.w - 20.0,
            text: title.clone(), px: 12.0, color, center: true,
        });
    }
    out
}

// ── state ───────────────────────────────────────────────────────────────────

use std::collections::HashMap;
use std::time::Instant;

use smithay::utils::{IsAlive, Point};

use crate::state::{window_id, Drag, State};

/// A grouped tile's tab strip as laid out by the last relayout.
#[derive(Clone)]
pub struct TabStrip {
    pub rect:    Rectangle<i32, Logical>,
    pub active:  Window,
    pub members: Vec<Window>,
}

/// A piece of chrome under the pointer.
#[derive(Clone, PartialEq)]
pub enum Hit {
    Tab  { active: Window, member: Window, idx: usize },
    Card { window: Window },
}

/// A press on chrome, held until release (click) or until it moves far enough
/// to become a drag (tab tear-off / pulling a card out of the shelf).
#[derive(Clone)]
pub struct Press {
    pub hit:    Hit,
    pub button: u32,
    pub at:     Point<f64, Logical>,
    pub shift:  bool,
}

#[derive(Default)]
pub struct ChromeState {
    /// Tab strips of the active desk (hit-testing + drop targets).
    pub strips:        Vec<TabStrip>,
    /// Shelf hidden by the user (super+minus). It only shows when it holds
    /// windows AND isn't hidden.
    pub shelf_hidden:  bool,
    /// Last show/hide flip — drives the slide.
    pub shelf_toggled: Option<Instant>,
    /// Shelf column (global logical) and its cards, from the last relayout.
    /// Kept while hidden so the slide-out has somewhere to slide from.
    pub shelf_rect:    Option<Rectangle<i32, Logical>>,
    pub cells:         Vec<(Window, Rectangle<i32, Logical>)>,
    /// Windows flying onto the shelf: (window, where it left from, when).
    pub fly:           Vec<(Window, Rectangle<i32, Logical>, Instant)>,
    pub hover:         Option<Hit>,
    pub press:         Option<Press>,
    /// Eased 0..1 hover amount per card (window id).
    pub card_hover:    HashMap<u32, f32>,
    /// A drag is hovering the shelf — dropping stashes the window.
    pub shelf_drop:    bool,
    /// Pinned windows: floating, on every desk, above everything else.
    pub pinned:        Vec<Window>,
    /// Pinned windows that were tiled before — unpinning puts them back.
    pub pinned_from_tile: Vec<Window>,
    /// Monitor the shelf lives on (the one you first stashed from).
    pub shelf_output:  Option<String>,
    /// Phone mirror windows (window rule `phone=#true`): borderless, rounded
    /// like a phone screen, sized to the stream's aspect. With the aspect they
    /// were last fitted to, to notice a rotation.
    pub phones:        Vec<(Window, f64)>,
    /// View-only phone windows (the AirPlay mirror): a bare left-drag moves
    /// them. Controllable ones (Android/scrcpy) keep their clicks.
    pub phone_drag:    Vec<Window>,
    /// vendi-phone-hid is connected: clicks/keys on the iPhone mirror go to
    /// the phone (as a Bluetooth mouse/keyboard) instead of moving the window.
    pub phone_control: bool,
    /// A press on the phone mirror is in flight (its button).
    pub phone_press:   Option<u32>,
    /// The pointer is over the iPhone mirror (phone control on): the phone's
    /// own pointer follows it.
    pub phone_hover:   bool,
    /// Carries a focused pinned window's focus across a desk switch.
    pub pin_focus:     Option<Window>,
    /// Last frame's render clock, for the eased hover.
    pub last_frame:    Option<Instant>,
}

const SLIDE_MS: f32 = 320.0;
const FLY_MS: f32 = 340.0;
const PULL_SLOP: f64 = 6.0;
const BTN_LEFT: u32 = 0x110;
const BTN_MIDDLE: u32 = 0x112;

fn ease_out(t: f32) -> f32 { 1.0 - (1.0 - t).powi(3) }
fn ease_out_quint(t: f32) -> f32 { 1.0 - (1.0 - t).powi(5) }

fn lerp_rect(a: Rectangle<i32, Logical>, b: Rectangle<i32, Logical>, e: f32) -> Rectangle<f64, Logical> {
    let l = |x: i32, y: i32| x as f64 + (y - x) as f64 * e as f64;
    Rectangle::new(
        (l(a.loc.x, b.loc.x), l(a.loc.y, b.loc.y)).into(),
        (l(a.size.w, b.size.w), l(a.size.h, b.size.h)).into(),
    )
}

impl State {
    pub fn shelf_visible(&self) -> bool {
        !self.workspaces.scratchpad.is_empty() && !self.chrome.shelf_hidden
    }

    /// The monitor the shelf is on: where it was started, while that monitor
    /// is connected; otherwise the focused one.
    pub fn shelf_output_name(&self) -> Option<String> {
        self.chrome.shelf_output.clone()
            .filter(|n| self.output_named(n).is_some())
            .or_else(|| self.focused_output().map(|o| o.name()))
    }

    /// The shelf column: left of the tiling area, inside the outer margin,
    /// below the bar. None without an output.
    pub fn shelf_region(&self) -> Option<Rectangle<i32, Logical>> {
        let output = self.output_named(&self.shelf_output_name()?)?;
        let geometry = self.space.output_geometry(&output)?;
        let zone = smithay::desktop::layer_map_for_output(&output).non_exclusive_zone();
        let m = self.config.theme.margin;
        Some(Rectangle::new(
            (geometry.loc.x + zone.loc.x + m, geometry.loc.y + zone.loc.y + m).into(),
            (SHELF_W - self.config.theme.gap, (zone.size.h - m * 2).max(1)).into(),
        ))
    }

    /// Lay the shelf cards out: a centered column, each card the window's
    /// aspect, shrinking together when there are many.
    pub fn layout_shelf(&mut self) {
        let Some(region) = self.shelf_region() else { return };
        self.chrome.shelf_rect = Some(region);
        let wins: Vec<Window> = self.workspaces.scratchpad.iter().filter(|w| w.alive()).cloned().collect();
        const LABEL: f64 = 22.0;
        const SPACING: f64 = 14.0;
        let cw = (region.size.w - 28) as f64;
        let aspects: Vec<f64> = wins.iter().map(|w| {
            let g = w.geometry().size;
            let g = if g.w > 0 && g.h > 0 { g }
                    else { self.last_geos.get(&window_id(w)).map(|r| r.size).unwrap_or((16, 10).into()) };
            (g.h as f64 / g.w.max(1) as f64).clamp(0.4, 1.4)
        }).collect();
        let heights: Vec<f64> = aspects.iter().map(|a| (cw * a).clamp(64.0, 170.0)).collect();
        let total: f64 = heights.iter().map(|h| h + LABEL).sum::<f64>()
            + SPACING * (wins.len().saturating_sub(1)) as f64;
        let avail = (region.size.h - 32) as f64;
        let s = if total > avail { avail / total } else { 1.0 };
        let mut y = region.loc.y as f64 + (region.size.h as f64 - total * s) / 2.0;
        let cx = region.loc.x as f64 + region.size.w as f64 / 2.0;
        self.chrome.cells = wins.into_iter().zip(heights).map(|(w, h)| {
            let (ww, hh) = (cw * s, h * s);
            let r = Rectangle::new(
                ((cx - ww / 2.0).round() as i32, y.round() as i32).into(),
                (ww.round() as i32, hh.round() as i32).into(),
            );
            y += (h + LABEL + SPACING) * s;
            (w, r)
        }).collect();
    }

    /// Title for chrome: the tracked xdg title, the X11 title, or the app id.
    pub fn chrome_title(&self, w: &Window) -> String {
        if let Some(t) = self.window_titles.get(&window_id(w)).filter(|t| !t.is_empty()) {
            return t.clone();
        }
        #[cfg(feature = "xwayland")]
        if let Some(x) = w.x11_surface() {
            let t = x.title();
            if !t.is_empty() { return t; }
            return x.class();
        }
        if let Some(t) = w.toplevel() {
            let app = smithay::wayland::compositor::with_states(t.wl_surface(), |s| {
                s.data_map.get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                    .and_then(|d| d.lock().ok().and_then(|d| d.app_id.clone()))
            });
            if let Some(a) = app { return a; }
        }
        String::from("window")
    }

    // ── hit-testing ─────────────────────────────────────────────────────────

    fn chrome_live(&self) -> bool {
        !self.overview && !self.vlock && !self.is_locked()
            && self.workspaces.active_ref().fullscreen.is_none()
    }

    pub fn chrome_hit(&self, pos: Point<f64, Logical>) -> Option<Hit> {
        if !self.chrome_live() { return None; }
        for s in &self.chrome.strips {
            if !s.rect.to_f64().contains(pos) { continue; }
            let local = pos.x - s.rect.loc.x as f64;
            let slots = tab_slots(s.rect.size.w as f64, s.members.len());
            let idx = slots.iter().position(|r| local >= r.loc.x && local <= r.loc.x + r.size.w)
                .unwrap_or_else(|| {
                    // between slots: nearest one
                    slots.iter().enumerate().min_by(|(_, a), (_, b)| {
                        let da = (a.loc.x + a.size.w / 2.0 - local).abs();
                        let db = (b.loc.x + b.size.w / 2.0 - local).abs();
                        da.total_cmp(&db)
                    }).map(|(i, _)| i).unwrap_or(0)
                });
            let member = s.members.get(idx)?.clone();
            return Some(Hit::Tab { active: s.active.clone(), member, idx });
        }
        if self.shelf_visible() {
            for (w, r) in &self.chrome.cells {
                // generous: the card plus its label
                let hit = Rectangle::<i32, Logical>::new(r.loc, (r.size.w, r.size.h + 22).into());
                if hit.to_f64().contains(pos) {
                    return Some(Hit::Card { window: w.clone() });
                }
            }
        }
        None
    }

    fn set_chrome_hover(&mut self, hit: Option<Hit>) {
        if self.chrome.hover != hit {
            self.chrome.hover = hit;
            self.pending_redraw = true;
        }
    }

    // ── pointer entry points (called by the backends) ───────────────────────

    /// Pointer moved. Returns true if chrome owns the motion (a press on a tab
    /// or card is in flight) and clients must not see it.
    pub fn chrome_motion(&mut self) -> bool {
        let pos = self.pointer_location;
        if let Some(p) = self.chrome.press.clone() {
            let d = ((pos.x - p.at.x).powi(2) + (pos.y - p.at.y).powi(2)).sqrt();
            if p.button == BTN_LEFT && d > PULL_SLOP {
                self.chrome.press = None;
                match p.hit {
                    Hit::Tab { member, .. } => self.tear_off_tab(&member),
                    Hit::Card { window } => self.pull_from_shelf(&window),
                }
                return true;
            }
            return true;
        }
        let hit = self.chrome_hit(pos);
        self.set_chrome_hover(hit);
        false
    }

    /// Button press/release. Returns true if chrome consumed it.
    pub fn chrome_button(&mut self, code: u32, pressed: bool) -> bool {
        if pressed {
            let Some(hit) = self.chrome_hit(self.pointer_location) else { return false };
            let shift = self.seat.get_keyboard().map(|k| k.modifier_state().shift).unwrap_or(false);
            self.chrome.press = Some(Press { hit, button: code, at: self.pointer_location, shift });
            return true;
        }
        let Some(p) = self.chrome.press.take() else { return false };
        if p.button != code { self.chrome.press = Some(p); return true; }
        match (p.hit, code) {
            (Hit::Tab { member, .. }, BTN_LEFT) => self.activate_tab(&member),
            (Hit::Tab { member, .. }, BTN_MIDDLE) => close_window(&member),
            (Hit::Card { window }, BTN_LEFT) => {
                if p.shift { self.stage_add(&window) } else { self.stage_swap(&window) }
            }
            (Hit::Card { window }, BTN_MIDDLE) => close_window(&window),
            _ => {}
        }
        true
    }

    /// Wheel over a tab strip cycles its tabs. True if consumed.
    pub fn chrome_scroll(&mut self, v: f64) -> bool {
        if v.abs() < 0.01 { return false; }
        let Some(Hit::Tab { active, .. }) = self.chrome_hit(self.pointer_location) else { return false };
        let ws = self.workspaces.active_ref();
        let Some(si) = ws.stack_of(&active) else { return false };
        let s = &ws.stacks[si];
        let n = s.members.len();
        let next = if v > 0.0 { (s.active + 1) % n } else { (s.active + n - 1) % n };
        let target = s.members[next].clone();
        self.activate_tab(&target);
        true
    }

    // ── tabs ────────────────────────────────────────────────────────────────

    /// Bring `member` to the front of its group (it takes the tile's slot).
    pub fn activate_tab(&mut self, member: &Window) {
        let ws = self.workspaces.active();
        let Some(si) = ws.stack_of(member) else { return };
        let Some(idx) = ws.stacks[si].members.iter().position(|w| w == member) else { return };
        let old = ws.stacks[si].members[ws.stacks[si].active].clone();
        ws.focus_floating = None;
        if old != *member {
            ws.stacks[si].active = idx;
            ws.tree.replace_window(&old, member);
            self.space.unmap_elem(&old);
            // the incoming tab fades up in the slot rather than popping
            self.open_anims.retain(|(x, _)| x != member);
            self.open_anims.push((member.clone(), None));
        }
        self.workspaces.active().tree.focus_window(member);
        self.relayout();
        self.update_keyboard_focus();
        self.emit_workspaces();
    }

    /// Take `member` out of its group, leaving the rest of the group in place.
    /// Returns false if it wasn't grouped. The window ends up off-tree.
    fn detach_from_group(&mut self, member: &Window) -> bool {
        let ws = self.workspaces.active();
        let Some(si) = ws.stack_of(member) else { return false };
        let s = &mut ws.stacks[si];
        let idx = s.members.iter().position(|w| w == member).unwrap();
        let was_active = idx == s.active;
        s.members.remove(idx);
        if was_active {
            // next member steps into the slot
            s.active = idx.min(s.members.len() - 1);
            let heir = s.members[s.active].clone();
            ws.tree.replace_window(member, &heir);
            ws.tree.focus_window(&heir);
        } else if idx < s.active {
            s.active -= 1;
        }
        ws.prune_stacks();
        true
    }

    /// Public wrapper: take a window out of its tab group (if any).
    pub fn detach_group_member(&mut self, w: &Window) { self.detach_from_group(w); }

    /// Drag a tab out of its strip: it leaves the group and follows the cursor
    /// like any Super+dragged tile (live drop preview, re-tiles on release).
    fn tear_off_tab(&mut self, member: &Window) {
        let size = self.chrome.strips.iter()
            .find(|s| s.members.contains(member))
            .map(|s| s.rect.size)
            .unwrap_or((640, 420).into());
        if !self.detach_from_group(member) { return; }
        let h = self.space.element_geometry(member).map(|g| g.size.h)
            .or_else(|| self.chrome.strips.iter().find(|s| s.members.contains(member))
                .and_then(|s| self.tile_geos.get(&window_id(&s.active)).map(|r| r.size.h)))
            .unwrap_or(420);
        let w = (size.w * 3 / 4).max(320);
        let h = (h * 3 / 4).max(220);
        self.begin_pull(member, (w, h).into());
    }

    /// Put `w` in the active desk's floating layer centered under the cursor,
    /// mapped, and start a tile drag on it (drops re-tile it).
    fn begin_pull(&mut self, w: &Window, size: smithay::utils::Size<i32, Logical>) {
        let pos = self.pointer_location;
        let rect = Rectangle::new(
            ((pos.x as i32) - size.w / 2, (pos.y as i32) - 18).into(),
            size,
        );
        self.begin_pull_at(w, rect);
    }

    /// Super+drag on a grouped tile: the window leaves its group and is picked
    /// up in place (its siblings keep the slot).
    pub fn pull_grouped(&mut self, w: &Window) -> bool {
        if self.workspaces.active_ref().stack_of(w).is_none() { return false; }
        let geo = self.space.element_geometry(w);
        self.detach_from_group(w);
        let pos = self.pointer_location;
        let rect = geo.unwrap_or_else(|| Rectangle::new(
            (pos.x as i32 - 300, pos.y as i32 - 20).into(), (600, 400).into()));
        self.begin_pull_at(w, rect);
        true
    }

    fn begin_pull_at(&mut self, w: &Window, rect: Rectangle<i32, Logical>) {
        let pos = self.pointer_location;
        let size = rect.size;
        let ws = self.workspaces.active();
        ws.floating.retain(|(x, _)| x != w);
        ws.floating.push((w.clone(), rect));
        ws.focus_floating = Some(w.clone());
        if let Some(t) = w.toplevel() {
            t.with_pending_state(|s| { s.size = Some(size); });
            t.send_pending_configure();
        }
        #[cfg(feature = "xwayland")]
        if let Some(x) = w.x11_surface() { let _ = x.configure(Some(rect)); }
        self.space.map_element(w.clone(), rect.loc, true);
        let now = Instant::now();
        self.drag = Some(Drag {
            window: w.clone(), resize: false, tile_resize: false, from_tile: true,
            start_ptr: pos, start_rect: rect, started: now, last_apply: now,
            vel: (0.0, 0.0), last_motion: now, last_ptr: pos,
        });
        self.set_chrome_hover(None);
        self.relayout();
        self.update_keyboard_focus();
    }

    /// Group `dragged` into the tile holding `target` (drop onto a tab strip
    /// or a tile's middle). The dragged window becomes the front tab.
    pub fn join_group(&mut self, target: &Window, dragged: &Window) {
        if target == dragged { return; }
        // a window lives in at most one group
        self.detach_from_group(dragged);
        let ws = self.workspaces.active();
        ws.floating.retain(|(w, _)| w != dragged);
        if ws.focus_floating.as_ref() == Some(dragged) { ws.focus_floating = None; }
        ws.tree.remove(dragged);
        if let Some(si) = ws.stack_of(target) {
            let old = ws.stacks[si].members[ws.stacks[si].active].clone();
            ws.stacks[si].members.push(dragged.clone());
            ws.stacks[si].active = ws.stacks[si].members.len() - 1;
            ws.tree.replace_window(&old, dragged);
            self.space.unmap_elem(&old);
        } else {
            if !ws.tree.contains(target) { return; }
            ws.tree.replace_window(target, dragged);
            ws.stacks.push(crate::workspaces::Stack {
                members: vec![target.clone(), dragged.clone()], active: 1,
            });
            self.space.unmap_elem(target);
        }
        self.workspaces.active().tree.focus_window(dragged);
    }

    // ── shelf (stage) ───────────────────────────────────────────────────────

    /// super+shift+minus / dropping on the shelf: stash `w` onto the shelf at
    /// `at` (0 = top). It flies from where it was to its card.
    pub fn stash(&mut self, w: &Window, at: usize) {
        let from = self.space.element_geometry(w)
            .or_else(|| self.last_geos.get(&window_id(w)).copied());
        if self.workspaces.active_ref().fullscreen.as_ref() == Some(w) {
            self.set_fullscreen(w, false);
        }
        self.detach_from_group(w);
        self.workspaces.detach_everywhere(w);
        self.chrome.pinned.retain(|p| p != w);
        self.chrome.pinned_from_tile.retain(|p| p != w);
        self.space.unmap_elem(w);
        self.workspaces.scratchpad.retain(|x| x != w);
        let at = at.min(self.workspaces.scratchpad.len());
        // an empty shelf opens on the monitor you're stashing from
        if self.workspaces.scratchpad.is_empty() {
            self.chrome.shelf_output = self.focused_output().map(|o| o.name());
        }
        self.workspaces.scratchpad.insert(at, w.clone());
        if self.chrome.shelf_hidden {
            self.chrome.shelf_hidden = false;
            self.chrome.shelf_toggled = Some(Instant::now());
        } else if self.workspaces.scratchpad.len() == 1 {
            self.chrome.shelf_toggled = Some(Instant::now());
        }
        if let Some(from) = from {
            self.chrome.fly.retain(|(x, _, _)| x != w);
            self.chrome.fly.push((w.clone(), from, Instant::now()));
        }
        self.relayout();
        self.update_keyboard_focus();
        self.emit_workspaces();
    }

    /// Click a card: that window takes the focused window's place on stage,
    /// and the focused window goes to the shelf where the card was — Stage
    /// Manager's swap. On an empty desk it just comes on stage.
    pub fn stage_swap(&mut self, w: &Window) {
        let Some(idx) = self.workspaces.scratchpad.iter().position(|x| x == w) else { return };
        let card = self.chrome.cells.iter().find(|(x, _)| x == w).map(|(_, r)| *r);
        let out = self.workspaces.active_ref().focused_window()
            .filter(|f| !self.chrome.pinned.contains(f));
        self.workspaces.scratchpad.remove(idx);
        let ws = self.workspaces.active();
        match &out {
            Some(o) if ws.tree.contains(o) => {
                let from = self.space.element_geometry(o);
                if let Some(si) = ws.stack_of(o) {
                    // swap just that tab: the card joins the group in its place
                    let s = &mut ws.stacks[si];
                    let i = s.members.iter().position(|m| m == o).unwrap();
                    s.members[i] = w.clone();
                }
                ws.tree.replace_window(o, w);
                ws.tree.focus_window(w);
                self.space.unmap_elem(o);
                self.workspaces.scratchpad.insert(idx, o.clone());
                if let Some(from) = from {
                    self.chrome.fly.retain(|(x, _, _)| x != o);
                    self.chrome.fly.push((o.clone(), from, Instant::now()));
                }
            }
            Some(o) if ws.floating.iter().any(|(x, _)| x == o) => {
                let from = self.space.element_geometry(o);
                if let Some(e) = ws.floating.iter_mut().find(|(x, _)| x == o) { e.0 = w.clone(); }
                ws.focus_floating = Some(w.clone());
                self.space.unmap_elem(o);
                self.workspaces.scratchpad.insert(idx, o.clone());
                if let Some(from) = from {
                    self.chrome.fly.retain(|(x, _, _)| x != o);
                    self.chrome.fly.push((o.clone(), from, Instant::now()));
                }
            }
            _ => {
                ws.focus_floating = None;
                ws.tree.insert(w.clone());
            }
        }
        self.bring_on_stage(w, card);
    }

    /// Shift+click a card: add that window to the stage next to the focused
    /// one, leaving everything else where it is.
    pub fn stage_add(&mut self, w: &Window) {
        let Some(idx) = self.workspaces.scratchpad.iter().position(|x| x == w) else { return };
        let card = self.chrome.cells.iter().find(|(x, _)| x == w).map(|(_, r)| *r);
        self.workspaces.scratchpad.remove(idx);
        let ws = self.workspaces.active();
        ws.focus_floating = None;
        ws.tree.insert(w.clone());
        self.bring_on_stage(w, card);
    }

    /// Keyboard: swap the top card on stage (super+equal).
    pub fn stage_pull_next(&mut self) {
        if let Some(w) = self.workspaces.scratchpad.first().cloned() {
            if self.chrome.shelf_hidden {
                self.chrome.shelf_hidden = false;
                self.chrome.shelf_toggled = Some(Instant::now());
            }
            self.stage_swap(&w);
        }
    }

    /// Relayout, then glide `w` from its card to its new slot.
    fn bring_on_stage(&mut self, w: &Window, card: Option<Rectangle<i32, Logical>>) {
        self.chrome.fly.retain(|(x, _, _)| x != w);
        if self.workspaces.scratchpad.is_empty() {
            self.chrome.shelf_toggled = Some(Instant::now());
        }
        self.relayout();
        if let Some(card) = card {
            self.geo_anims.retain(|(x, _, _)| x != w);
            self.geo_anims.push((w.clone(), card, Instant::now()));
        }
        self.update_keyboard_focus();
        self.emit_workspaces();
    }

    /// Pull a card out by dragging it: the window follows the cursor and
    /// lands wherever the drop preview shows (or back on the shelf).
    fn pull_from_shelf(&mut self, w: &Window) {
        let Some(idx) = self.workspaces.scratchpad.iter().position(|x| x == w) else { return };
        self.workspaces.scratchpad.remove(idx);
        let g = w.geometry().size;
        let vp = self.shelf_region().map(|r| r.size).unwrap_or((1280, 720).into());
        let size = if g.w > 0 && g.h > 0 {
            (g.w.min(vp.h * 3 / 2).max(320), g.h.min(vp.h * 3 / 4).max(220)).into()
        } else { (720, 460).into() };
        if self.workspaces.scratchpad.is_empty() {
            self.chrome.shelf_toggled = Some(Instant::now());
        }
        self.begin_pull(w, size);
    }

    /// super+minus: show/hide the shelf (the stage reclaims its space).
    pub fn toggle_shelf(&mut self) {
        if self.workspaces.scratchpad.is_empty() { return; }
        self.chrome.shelf_hidden = !self.chrome.shelf_hidden;
        self.chrome.shelf_toggled = Some(Instant::now());
        self.set_chrome_hover(None);
        self.relayout();
    }

    /// While dragging: is the pointer over the (visible) shelf?
    pub fn pointer_over_shelf(&self) -> bool {
        self.shelf_visible() && self.chrome.shelf_rect
            .map(|r| {
                // the whole column out to the screen edge
                let r = Rectangle::<i32, Logical>::new((r.loc.x - 200, r.loc.y).into(),
                    (r.size.w + 200, r.size.h).into());
                r.to_f64().contains(self.pointer_location)
            })
            .unwrap_or(false)
    }

    /// Shelf index a drop at the pointer would land at.
    pub fn shelf_drop_index(&self) -> usize {
        let y = self.pointer_location.y;
        self.chrome.cells.iter()
            .position(|(_, r)| y < (r.loc.y + r.size.h / 2) as f64)
            .unwrap_or(self.chrome.cells.len())
    }

    // ── pin (picture-in-picture) ────────────────────────────────────────────

    /// super+p: pin the focused window — it floats (a tiled window shrinks to
    /// a corner, picture-in-picture style), follows you to every desk, and
    /// stays above other windows. Again to unpin (it stays floating here).
    pub fn toggle_pin(&mut self) {
        let Some(w) = self.focused_window() else { return };
        if let Some(i) = self.chrome.pinned.iter().position(|p| p == &w) {
            // Unpin: a window that came from the tiling goes back into it (it
            // glides from the corner to its new slot); a floating one stays put.
            self.chrome.pinned.remove(i);
            if let Some(j) = self.chrome.pinned_from_tile.iter().position(|p| p == &w) {
                self.chrome.pinned_from_tile.remove(j);
                let ws = self.workspaces.active();
                ws.floating.retain(|(x, _)| x != &w);
                ws.focus_floating = None;
                ws.tree.insert(w.clone());
            }
            self.relayout();
            self.update_keyboard_focus();
            return;
        }
        let tiled = self.workspaces.active_ref().tree.contains(&w);
        if tiled {
            self.detach_from_group(&w);
            let Some(region) = self.tiling_viewport_pub() else { return };
            let Some(output) = self.focused_output() else { return };
            let Some(og) = self.space.output_geometry(&output) else { return };
            let cur = self.space.element_geometry(&w).map(|g| g.size).unwrap_or((800, 450).into());
            let wdt = (og.size.w * 3 / 10).max(360);
            let hgt = ((wdt as f64 * cur.h as f64 / cur.w.max(1) as f64) as i32)
                .clamp(200, og.size.h / 2);
            let m = self.config.theme.margin + self.config.theme.gap;
            let bottom = region.loc.y + region.size.h;
            let rect = Rectangle::new(
                (og.loc.x + og.size.w - wdt - m, bottom - hgt).into(),
                (wdt, hgt).into(),
            );
            let ws = self.workspaces.active();
            ws.tree.remove(&w);
            ws.floating.push((w.clone(), rect));
            ws.focus_floating = Some(w.clone());
        } else if !self.workspaces.active_ref().floating.iter().any(|(x, _)| x == &w) {
            return;
        }
        if tiled { self.chrome.pinned_from_tile.push(w.clone()); }
        self.chrome.pinned.push(w);
        self.relayout();
        self.update_keyboard_focus();
    }

    /// Before a desk switch: lift pinned windows off the old desk (so they
    /// aren't hidden with it) — `repin` drops them onto the new one.
    pub fn unpin_for_switch(&mut self) -> Vec<(Window, Rectangle<i32, Logical>)> {
        self.chrome.pinned.retain(|w| w.alive());
        let pins = self.chrome.pinned.clone();
        let ws = self.workspaces.active();
        let mut out = Vec::new();
        ws.floating.retain(|(w, r)| {
            if pins.contains(w) { out.push((w.clone(), *r)); false } else { true }
        });
        // a focused pin stays focused on the next desk (so super+p unpins it)
        self.chrome.pin_focus = ws.focus_floating.clone().filter(|f| pins.contains(f));
        if self.chrome.pin_focus.is_some() { ws.focus_floating = None; }
        out
    }

    pub fn repin(&mut self, pins: Vec<(Window, Rectangle<i32, Logical>)>) {
        let focus = self.chrome.pin_focus.take();
        let ws = self.workspaces.active();
        for (w, r) in pins { ws.floating.push((w, r)); }
        if let Some(f) = focus.filter(|f| ws.floating.iter().any(|(x, _)| x == f)) {
            ws.focus_floating = Some(f);
        }
    }

    // ── phone mirror windows ────────────────────────────────────────────────

    /// The window under a point, topmost first — like Space::element_under,
    /// except a phone mirror only counts inside the phone shape it's drawn
    /// as (its stream buffer can be larger than that, and an invisible margin
    /// around the phone used to swallow clicks meant for other windows).
    pub fn window_under(&self, pos: Point<f64, Logical>) -> Option<(Window, Point<i32, Logical>)> {
        use smithay::desktop::space::SpaceElement;
        for w in self.space.elements().rev() {
            let Some(loc) = self.space.element_location(w) else { continue };
            if self.is_phone(w) {
                let shape = self.workspaces.iter()
                    .flat_map(|ws| ws.floating.iter())
                    .find(|(x, _)| x == w).map(|(_, r)| *r);
                if shape.is_some_and(|r| r.to_f64().contains(pos)) { return Some((w.clone(), loc)); }
                continue;
            }
            let Some(bbox) = self.space.element_bbox(w) else { continue };
            if bbox.to_f64().contains(pos) && w.is_in_input_region(&(pos - loc.to_f64())) {
                return Some((w.clone(), loc));
            }
        }
        None
    }

    /// Keep "always on top" windows on top: pinned windows and phone mirrors
    /// (a view-only mirror never gets focus, so focus raises would bury it).
    pub fn raise_overlays(&mut self) {
        let tops: Vec<Window> = self.chrome.phones.iter().map(|(w, _)| w.clone())
            .chain(self.chrome.pinned.iter().cloned())
            .collect();
        for w in tops {
            if self.space.element_geometry(&w).is_some() { self.space.raise_element(&w, false); }
        }
    }

    pub fn is_phone(&self, w: &Window) -> bool {
        self.chrome.phones.iter().any(|(p, _)| p == w)
    }

    /// Window rule `phone=#true`: float it, drop the border, fit it.
    pub fn make_phone(&mut self, w: &Window) {
        if self.is_phone(w) { return; }
        // mark it first, so floating it never sends a size (see layout_desk)
        self.chrome.phones.push((w.clone(), 0.0));
        if self.workspaces.active_ref().tree.contains(w) { self.float_window(w); }
        if self.window_process(w).eq_ignore_ascii_case("uxplay") {
            self.chrome.phone_drag.push(w.clone());
        }
        self.fit_phone(w);
    }

    /// Size a phone window to its stream's aspect: 85% of the monitor's
    /// usable height (or 90% of its width for a landscape stream), centered.
    pub fn fit_phone(&mut self, w: &Window) {
        let g = w.geometry().size;
        if g.w <= 0 || g.h <= 0 { return; }   // no frame yet — fitted on its first commit
        let aspect = g.w as f64 / g.h as f64;
        let Some(vp) = self.tiling_viewport_pub() else { return };
        let (mut ww, mut hh) = ((vp.size.h as f64 * 0.85 * aspect), vp.size.h as f64 * 0.85);
        if ww > vp.size.w as f64 * 0.9 {
            ww = vp.size.w as f64 * 0.9;
            hh = ww / aspect;
        }
        let size: smithay::utils::Size<i32, Logical> = (ww.round() as i32, hh.round() as i32).into();
        // keep its position if it's already out there (a rotation), centred
        // on the same point; otherwise centre it on the monitor
        let ws = self.workspaces.active();
        let rect = match ws.floating.iter().find(|(x, _)| x == w).map(|(_, r)| *r) {
            Some(old) if self.chrome.phones.iter().any(|(p, a)| p == w && *a > 0.0) => Rectangle::new(
                (old.loc.x + old.size.w / 2 - size.w / 2, old.loc.y + old.size.h / 2 - size.h / 2).into(), size),
            _ => Rectangle::new(
                (vp.loc.x + (vp.size.w - size.w) / 2, vp.loc.y + (vp.size.h - size.h) / 2).into(), size),
        };
        let ws = self.workspaces.active();
        if let Some(e) = ws.floating.iter_mut().find(|(x, _)| x == w) { e.1 = rect; }
        if let Some(e) = self.chrome.phones.iter_mut().find(|(p, _)| p == w) { e.1 = aspect; }
        self.relayout();
    }

    /// A phone window committed: fit it on its first frame, and again when
    /// the stream's shape changes (the phone was rotated).
    pub fn phone_commit(&mut self, w: &Window) {
        let Some(last) = self.chrome.phones.iter().find(|(p, _)| p == w).map(|(_, a)| *a) else { return };
        let g = w.geometry().size;
        if g.w <= 0 || g.h <= 0 { return; }
        if self.drag.as_ref().is_some_and(|d| &d.window == w) { return; }
        let aspect = g.w as f64 / g.h as f64;
        if last <= 0.0 || (aspect / last - 1.0).abs() > 0.03 {
            self.fit_phone(w);
        }
    }

    /// The iPhone mirror under the pointer and where on the phone screen
    /// (0..1, 0..1) — for phone control.
    fn phone_point(&self) -> Option<(Window, f64, f64)> {
        let pos = self.pointer_location;
        let (w, _) = self.window_under(pos)?;
        if !self.chrome.phone_drag.contains(&w) { return None; }
        let r = self.workspaces.iter().flat_map(|ws| ws.floating.iter())
            .find(|(x, _)| x == &w).map(|(_, r)| *r)?;
        Some((w,
              ((pos.x - r.loc.x as f64) / r.size.w.max(1) as f64).clamp(0.0, 1.0),
              ((pos.y - r.loc.y as f64) / r.size.h.max(1) as f64).clamp(0.0, 1.0)))
    }

    /// Phone control: a button press/release on the mirror becomes a tap on
    /// the phone. True if consumed (the mirror window doesn't see it).
    pub fn phone_button(&mut self, code: u32, pressed: bool) -> bool {
        if !self.chrome.phone_control { return false; }
        // left, right, middle, side, extra → phone buttons 1–5 (iOS can map
        // 3–5 to Home / App Switcher / Control Center in AssistiveTouch)
        let button = match code { 0x110 => 1, 0x111 => 2, 0x112 => 3, 0x113 => 4, 0x114 => 5, _ => return false };
        if pressed {
            let Some((w, x, y)) = self.phone_point() else { return false };
            self.chrome.phone_press = Some(button);
            self.focus_window(&w);
            self.pending_ipc_events.push(crate::ipc::Event::PhonePointer {
                phase: "down".into(), x, y, button });
            return true;
        }
        // any release ends a press — a tap can never get stuck
        if self.chrome.phone_press.take().is_some() {
            let (x, y) = self.phone_point().map(|(_, x, y)| (x, y)).unwrap_or((-1.0, -1.0));
            self.pending_ipc_events.push(crate::ipc::Event::PhonePointer {
                phase: "up".into(), x, y, button });
            return true;
        }
        false
    }

    /// Phone control: pointer motion over the mirror moves the phone's own
    /// pointer (hover), dragging during a press is a swipe, and leaving the
    /// mirror is announced (the phone re-syncs its pointer on the way back).
    pub fn phone_motion(&mut self) -> bool {
        if !self.chrome.phone_control {
            self.chrome.phone_hover = false;
            return false;
        }
        let here = self.phone_point();
        if let Some(button) = self.chrome.phone_press {
            if let Some((_, x, y)) = here {
                self.pending_ipc_events.push(crate::ipc::Event::PhonePointer {
                    phase: "move".into(), x, y, button });
            }
            return true;
        }
        match here {
            Some((_, x, y)) => {
                let phase = if self.chrome.phone_hover { "hover" } else { "enter" };
                self.chrome.phone_hover = true;
                self.pending_ipc_events.push(crate::ipc::Event::PhonePointer {
                    phase: phase.into(), x, y, button: 0 });
            }
            None if self.chrome.phone_hover => {
                self.chrome.phone_hover = false;
                self.pending_ipc_events.push(crate::ipc::Event::PhonePointer {
                    phase: "leave".into(), x: -1.0, y: -1.0, button: 0 });
            }
            None => {}
        }
        false   // hovering doesn't swallow the motion
    }

    /// Phone control: the wheel over the mirror scrolls the phone.
    pub fn phone_scroll(&mut self, dx: f64, dy: f64) -> bool {
        if !self.chrome.phone_control || self.phone_point().is_none() { return false; }
        self.pending_ipc_events.push(crate::ipc::Event::PhoneScroll { dx, dy });
        true
    }

    /// Phone control: keys go to the phone only while the mirror has focus
    /// AND the pointer is over it — moving the mouse off the phone always
    /// gives the keyboard back to the laptop.
    pub fn phone_keys(&self) -> bool {
        self.chrome.phone_control
            && self.focused_window().is_some_and(|w| self.chrome.phone_drag.contains(&w))
            && self.phone_point().is_some()
    }

    /// A phone window under the pointer (a bare left-drag moves it — there's
    /// nothing to click in a view-only mirror).
    pub fn phone_under_pointer(&self) -> bool {
        if self.chrome.phone_control { return false; }   // clicks are taps now
        self.window_under(self.pointer_location)
            .is_some_and(|(w, _)| self.chrome.phone_drag.contains(&w))
    }

    // ── per-frame scene ─────────────────────────────────────────────────────

    /// Build this frame's chrome. Called by the backend before rendering.
    pub fn chrome_scene(&mut self) -> Scene {
        let now = Instant::now();
        let dt = self.chrome.last_frame.map(|t| now.duration_since(t).as_secs_f32().min(0.1)).unwrap_or(0.0);
        self.chrome.last_frame = Some(now);
        let mut scene = Scene::default();
        let pal = Palette::from_theme(&self.config.theme);
        let live = self.chrome_live();
        let focused = self.focused_window();

        // tab strips
        if live {
            for s in &self.chrome.strips {
                let hover = match &self.chrome.hover {
                    Some(Hit::Tab { active, idx, .. }) if active == &s.active => Some(*idx),
                    _ => None,
                };
                scene.tabs.insert(window_id(&s.active), TabInfo {
                    titles: s.members.iter().map(|m| self.chrome_title(m)).collect(),
                    active: s.members.iter().position(|m| m == &s.active).unwrap_or(0),
                    hover,
                    focused: focused.as_ref() == Some(&s.active),
                });
            }
        }

        // shelf slide: 0 = off-screen left, 1 = in place
        let slide = {
            let p = self.chrome.shelf_toggled
                .map(|t| (now.duration_since(t).as_secs_f32() * 1000.0 / SLIDE_MS).min(1.0))
                .unwrap_or(1.0);
            if p < 1.0 { scene.animating = true; }
            let e = ease_out_quint(p);
            if self.shelf_visible() { e } else { 1.0 - e }
        };
        self.chrome.fly.retain(|(w, _, t)| w.alive()
            && now.duration_since(*t).as_secs_f32() * 1000.0 < FLY_MS);
        if !self.chrome.fly.is_empty() { scene.animating = true; }

        let has_cards = !self.chrome.cells.is_empty();
        if has_cards && slide > 0.001 && !self.overview && !self.vlock {
            let off = -(SHELF_W as f64 + 24.0) * (1.0 - slide as f64);
            let a = slide;
            if self.chrome.shelf_drop {
                if let Some(r) = self.chrome.shelf_rect {
                    let r = r.to_f64();
                    scene.shelf.push(Item::Rect { rect: r, radius: 16.0, color: with_alpha(pal.accent, 0.10) });
                    scene.shelf.push(Item::Ring { rect: r, radius: 16.0, thickness: 2.0, color: with_alpha(pal.accent, 0.8) });
                }
            }
            let hovered_card = match &self.chrome.hover {
                Some(Hit::Card { window }) => Some(window_id(window)),
                _ => None,
            };
            let k = 1.0 - (-dt / 0.06).exp();
            let cells = self.chrome.cells.clone();
            for (w, cell) in &cells {
                let wid = window_id(w);
                let target = if hovered_card == Some(wid) { 1.0 } else { 0.0 };
                let h = self.chrome.card_hover.entry(wid).or_insert(0.0);
                *h += (target - *h) * k;
                if (*h - target).abs() > 0.01 { scene.animating = true; } else { *h = target; }
                let h = *h as f64;

                let (mut rect, radius) = match self.chrome.fly.iter().find(|(x, _, _)| x == w) {
                    Some((_, from, t)) => {
                        let e = ease_out(now.duration_since(*t).as_secs_f32() * 1000.0 / FLY_MS);
                        (lerp_rect(*from, *cell, e), self.config.theme.radius + (10.0 - self.config.theme.radius) * e)
                    }
                    None => (cell.to_f64(), 10.0),
                };
                rect.loc.x += off;
                let grow = 5.0 * h;
                rect.loc.x -= grow; rect.loc.y -= grow;
                rect.size.w += grow * 2.0; rect.size.h += grow * 2.0;

                // soft drop shadow, then the live window, then the ring
                let mut sh = rect;
                sh.loc.y += 4.0;
                scene.shelf.push(Item::Rect { rect: sh, radius: radius + 2.0, color: [0.0, 0.0, 0.0, 0.28 * a] });
                scene.shelf.push(Item::Thumb { window: w.clone(), rect, radius, alpha: a });
                let ring = [
                    pal.inactive[0] + (pal.accent[0] - pal.inactive[0]) * h as f32,
                    pal.inactive[1] + (pal.accent[1] - pal.inactive[1]) * h as f32,
                    pal.inactive[2] + (pal.accent[2] - pal.inactive[2]) * h as f32,
                    1.0,
                ];
                scene.shelf.push(Item::Ring { rect, radius, thickness: 1.5 + h as f32, color: with_alpha(ring, 0.9 * a) });
                scene.shelf.push(Item::Text {
                    x: rect.loc.x, cy: rect.loc.y + rect.size.h + 12.0, max_w: rect.size.w,
                    text: self.chrome_title(w), px: 11.0,
                    color: with_alpha(if h > 0.5 { pal.fg } else { pal.dim }, a), center: true,
                });
                scene.live.push(w.clone());
            }
        }
        scene
    }
}

pub fn close_window(w: &Window) {
    if let Some(t) = w.toplevel() { t.send_close(); }
    #[cfg(feature = "xwayland")]
    if let Some(x) = w.x11_surface() { let _ = x.close(); }
}
