// Dynamic workspaces.
//
// vendiOS design: workspaces are created on demand (switch or move-to) and
// pruned when they end up empty and inactive. Each workspace owns an i3-style
// tiling tree plus a floating layer; one window per workspace may be
// fullscreened (display override — it stays in its container).

use std::collections::HashMap;

use smithay::desktop::Window;
use smithay::utils::{IsAlive, Logical, Rectangle};

use crate::layout::Tree;

pub struct Workspace {
    pub id:       u32,
    pub tree:     Tree,
    /// Floating windows with their logical geometry, bottom-to-top.
    pub floating: Vec<(Window, Rectangle<i32, Logical>)>,
    /// If Some, this floating window has focus instead of the tree's leaf.
    pub focus_floating: Option<Window>,
    /// Display override: render this window over the whole output.
    pub fullscreen: Option<Window>,
}

impl Workspace {
    fn new(id: u32) -> Self {
        Self {
            id,
            tree: Tree::new(),
            floating: Vec::new(),
            focus_floating: None,
            fullscreen: None,
        }
    }

    pub fn windows(&self) -> Vec<Window> {
        let mut out = self.tree.windows();
        out.extend(self.floating.iter().map(|(w, _)| w.clone()));
        out
    }

    pub fn is_empty(&self) -> bool {
        self.tree.is_empty() && self.floating.is_empty()
    }

    /// The window that should hold keyboard focus on this workspace.
    pub fn focused_window(&self) -> Option<Window> {
        if let Some(w) = &self.focus_floating {
            if w.alive() { return Some(w.clone()); }
        }
        self.tree.focused().cloned()
    }

    pub fn contains(&self, window: &Window) -> bool {
        self.tree.contains(window) || self.floating.iter().any(|(w, _)| w == window)
    }

    pub fn remove(&mut self, window: &Window) {
        self.tree.remove(window);
        self.floating.retain(|(w, _)| w != window);
        if self.focus_floating.as_ref() == Some(window) { self.focus_floating = None; }
        if self.fullscreen.as_ref() == Some(window) { self.fullscreen = None; }
    }

    /// Drop dead windows from both layers.
    pub fn prune_dead(&mut self) {
        self.tree.prune_dead();
        self.floating.retain(|(w, _)| w.alive());
        if let Some(w) = &self.focus_floating { if !w.alive() { self.focus_floating = None; } }
        if let Some(w) = &self.fullscreen { if !w.alive() { self.fullscreen = None; } }
    }
}

pub struct Workspaces {
    list:   Vec<Workspace>,   // sorted by id
    active: u32,
}

impl Workspaces {
    pub fn new() -> Self {
        Self { list: vec![Workspace::new(1)], active: 1 }
    }

    pub fn active_id(&self) -> u32 { self.active }

    pub fn active(&mut self) -> &mut Workspace {
        let id = self.active;
        self.get_mut(id)
    }

    pub fn active_ref(&self) -> &Workspace {
        self.list.iter().find(|w| w.id == self.active)
            .expect("active workspace always exists")
    }

    fn get_mut(&mut self, id: u32) -> &mut Workspace {
        if !self.list.iter().any(|w| w.id == id) {
            self.list.push(Workspace::new(id));
            self.list.sort_by_key(|w| w.id);
        }
        self.list.iter_mut().find(|w| w.id == id).unwrap()
    }

    /// Switch the active workspace (creating it if needed). Returns the
    /// windows that must be hidden (old workspace) — caller unmaps them.
    pub fn switch_to(&mut self, id: u32) -> Vec<Window> {
        if id == self.active { return Vec::new(); }
        let hidden = self.active_ref().windows();
        let _ = self.get_mut(id);   // ensure it exists
        self.active = id;
        self.prune_empty();
        hidden
    }

    /// Adjacent existing workspace (for 3-finger swipe). `forward` may step
    /// one past the last workspace to spawn a fresh one, GNOME-style, but
    /// only if the current one isn't already empty.
    pub fn adjacent_id(&self, forward: bool) -> Option<u32> {
        let ids: Vec<u32> = self.list.iter().map(|w| w.id).collect();
        let pos = ids.iter().position(|&i| i == self.active)?;
        if forward {
            if pos + 1 < ids.len() { return Some(ids[pos + 1]); }
            if !self.active_ref().is_empty() { return Some(self.active + 1); }
            None
        } else {
            if pos > 0 { Some(ids[pos - 1]) } else { None }
        }
    }

    /// Move `window` from whichever workspace holds it onto `id` (tiled).
    pub fn move_window_to(&mut self, window: &Window, id: u32) {
        for ws in &mut self.list {
            if ws.contains(window) { ws.remove(window); }
        }
        self.get_mut(id).tree.insert(window.clone());
        self.prune_empty();
    }

    /// Remove a window wherever it lives.
    pub fn remove_window(&mut self, window: &Window) {
        for ws in &mut self.list {
            ws.remove(window);
        }
        self.prune_empty();
    }

    pub fn find_workspace(&self, window: &Window) -> Option<u32> {
        self.list.iter().find(|w| w.contains(window)).map(|w| w.id)
    }

    pub fn prune_dead(&mut self) {
        for ws in &mut self.list { ws.prune_dead(); }
        self.prune_empty();
    }

    /// Drop empty inactive workspaces (dynamic policy). Workspace 1 and the
    /// active one always survive.
    fn prune_empty(&mut self) {
        let active = self.active;
        self.list.retain(|w| w.id == active || w.id == 1 || !w.is_empty());
    }

    /// Snapshot for IPC / the bar: (id, window-count) pairs plus active id.
    pub fn snapshot(&self) -> (u32, Vec<(u32, usize)>) {
        let mut counts: HashMap<u32, usize> = HashMap::new();
        for ws in &self.list {
            counts.insert(ws.id, ws.windows().len());
        }
        let mut out: Vec<(u32, usize)> = counts.into_iter().collect();
        out.sort_by_key(|(id, _)| *id);
        (self.active, out)
    }

    /// All windows across all workspaces.
    pub fn all_windows(&self) -> Vec<Window> {
        self.list.iter().flat_map(|w| w.windows()).collect()
    }

    /// All workspaces, sorted by id.
    pub fn iter(&self) -> impl Iterator<Item = &Workspace> {
        self.list.iter()
    }
}

impl Default for Workspaces {
    fn default() -> Self { Self::new() }
}
