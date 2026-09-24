// Dynamic workspaces, one shown per monitor.
//
// vendiOS design: workspaces are created on demand (switch or move-to) and
// pruned when they end up empty and not on screen. Each workspace owns an
// i3-style tiling tree plus a floating layer; one window per workspace may be
// fullscreened (display override — it stays in its container).
//
// Multi-monitor (sway-style): every output shows exactly one workspace
// (`shown`), and each workspace remembers the output it belongs to. `active`
// is the workspace on the FOCUSED output — the monitor under the pointer — so
// every "active workspace" call in the compositor means "where you're working".

use smithay::desktop::Window;
use smithay::utils::{IsAlive, Logical, Rectangle};

use crate::layout::{LayoutMode, Tree};

/// A tabbed stack: several windows sharing one tile, only `active` shown. The
/// active member lives in the tree as a normal leaf; the rest sit here off-tree
/// and unmapped. Cycling swaps which member occupies the slot (Tree::replace_window).
pub struct Stack {
    pub members: Vec<Window>,
    pub active:  usize,
}

impl Stack {
    pub fn active_window(&self) -> Option<&Window> { self.members.get(self.active) }
}

pub struct Workspace {
    pub id:       u32,
    pub tree:     Tree,
    /// Floating windows with their logical geometry, bottom-to-top.
    pub floating: Vec<(Window, Rectangle<i32, Logical>)>,
    /// If Some, this floating window has focus instead of the tree's leaf.
    pub focus_floating: Option<Window>,
    /// Display override: render this window over the whole output.
    pub fullscreen: Option<Window>,
    /// How the tiled windows are arranged (BSP split / master-stack / monocle).
    pub mode:     LayoutMode,
    /// Tabbed stacks living on this desk (each occupies one tree leaf).
    pub stacks:   Vec<Stack>,
    /// Output (connector name) this desk lives on. Empty = not yet placed.
    pub output:   String,
}

impl Workspace {
    fn new(id: u32, output: String) -> Self {
        Self {
            id,
            output,
            tree: Tree::new(),
            floating: Vec::new(),
            focus_floating: None,
            fullscreen: None,
            mode: LayoutMode::Tiling,
            stacks: Vec::new(),
        }
    }

    /// Index of the stack containing `window` (active or hidden member), if any.
    pub fn stack_of(&self, window: &Window) -> Option<usize> {
        self.stacks.iter().position(|s| s.members.iter().any(|w| w == window))
    }

    pub fn windows(&self) -> Vec<Window> {
        let mut out = self.tree.windows();
        out.extend(self.floating.iter().map(|(w, _)| w.clone()));
        out
    }

    pub fn is_empty(&self) -> bool {
        self.tree.is_empty() && self.floating.is_empty() && self.stacks.is_empty()
    }

    /// The window that should hold keyboard focus on this workspace.
    pub fn focused_window(&self) -> Option<Window> {
        // A fullscreen window (e.g. a game) covers everything and owns the
        // keyboard — otherwise focus drifts to a backgrounded tile and the game
        // gets no keys (transient launcher/Steam popups kept stealing it).
        if let Some(w) = &self.fullscreen {
            if w.alive() { return Some(w.clone()); }
        }
        if let Some(w) = &self.focus_floating {
            if w.alive() { return Some(w.clone()); }
        }
        self.tree.focused().cloned()
    }

    /// Does this desk own `window`? Must consider stack members: only a
    /// stack's *active* member holds a tree slot, so a tree/floating-only
    /// check answers "no" for every hidden tab.
    pub fn contains(&self, window: &Window) -> bool {
        self.tree.contains(window)
            || self.floating.iter().any(|(w, _)| w == window)
            || self.stacks.iter().any(|s| s.members.iter().any(|w| w == window))
    }

    pub fn remove(&mut self, window: &Window) {
        self.tree.remove(window);
        self.floating.retain(|(w, _)| w != window);
        // Drop it from any stack too, or the desk still claims it: prune_stacks
        // would find the stack's active member missing from the tree and
        // reinsert it here — while the window also lives on its new desk, so it
        // renders on both at once.
        for s in &mut self.stacks { s.members.retain(|w| w != window); }
        self.prune_stacks();
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

    /// Keep tabbed stacks consistent: drop dead members, promote a live member
    /// into the tree if the active one closed, and dissolve singletons.
    pub fn prune_stacks(&mut self) {
        let mut i = 0;
        while i < self.stacks.len() {
            self.stacks[i].members.retain(|w| w.alive());
            if self.stacks[i].members.is_empty() { self.stacks.remove(i); continue; }
            if self.stacks[i].active >= self.stacks[i].members.len() {
                self.stacks[i].active = 0;
            }
            // The active member must occupy a tree slot; if its slot vanished
            // (previous active closed), reinsert it so the stack stays visible.
            let active = self.stacks[i].members[self.stacks[i].active].clone();
            if !self.tree.contains(&active)
                && !self.floating.iter().any(|(w, _)| w == &active)
            {
                self.tree.insert(active);
            }
            // A one-member stack is just a normal window — dissolve it.
            if self.stacks[i].members.len() < 2 { self.stacks.remove(i); continue; }
            i += 1;
        }
    }
}

pub struct Workspaces {
    list:   Vec<Workspace>,   // sorted by id
    active: u32,
    /// The workspace active before the current one — for back-and-forth.
    previous: u32,
    /// Stage shelf (the scratchpad): windows parked off every desk, shown as
    /// live cards down the left edge. Top card first.
    pub scratchpad: Vec<Window>,
    /// (output name, workspace id) — the desk each monitor is showing.
    shown: Vec<(String, u32)>,
}

impl Workspaces {
    pub fn new() -> Self {
        Self {
            list: vec![Workspace::new(1, String::new())], active: 1, previous: 1,
            scratchpad: Vec::new(),
            shown: Vec::new(),
        }
    }

    /// Remove a window from every desk's tree + floating layers (used by the
    /// scratchpad to detach a window before stashing or re-showing it).
    pub fn detach_everywhere(&mut self, window: &Window) {
        for ws in &mut self.list { ws.remove(window); }
    }

    pub fn active_id(&self) -> u32 { self.active }

    /// The previously-active workspace id (i3 back-and-forth target).
    pub fn previous_id(&self) -> u32 { self.previous }

    pub fn active(&mut self) -> &mut Workspace {
        let id = self.active;
        self.get_mut(id)
    }

    pub fn active_ref(&self) -> &Workspace {
        self.list.iter().find(|w| w.id == self.active)
            .expect("active workspace always exists")
    }

    /// Look up a workspace (None if it doesn't exist).
    pub fn get(&self, id: u32) -> Option<&Workspace> {
        self.list.iter().find(|w| w.id == id)
    }

    /// Create-on-demand. A new desk belongs to the focused monitor.
    fn get_mut(&mut self, id: u32) -> &mut Workspace {
        if !self.list.iter().any(|w| w.id == id) {
            let out = self.focused_output().to_string();
            self.list.push(Workspace::new(id, out));
            self.list.sort_by_key(|w| w.id);
        }
        self.list.iter_mut().find(|w| w.id == id).unwrap()
    }

    pub fn get_mut_existing(&mut self, id: u32) -> Option<&mut Workspace> {
        self.list.iter_mut().find(|w| w.id == id)
    }

    // ── monitors ────────────────────────────────────────────────────────────

    /// Connector name of the focused monitor ("" before any output exists).
    pub fn focused_output(&self) -> &str {
        self.get(self.active).map(|w| w.output.as_str()).unwrap_or("")
    }

    /// (output, workspace id) for every monitor, in plug-in order.
    pub fn shown(&self) -> &[(String, u32)] { &self.shown }

    /// The desk a monitor is showing.
    pub fn shown_on(&self, output: &str) -> Option<u32> {
        self.shown.iter().find(|(o, _)| o == output).map(|(_, id)| *id)
    }

    /// Is this desk on screen (on any monitor)?
    pub fn is_shown(&self, id: u32) -> bool {
        self.shown.iter().any(|(_, w)| *w == id)
    }

    /// The monitor a desk is on screen on, if it is.
    pub fn output_showing(&self, id: u32) -> Option<&str> {
        self.shown.iter().find(|(_, w)| *w == id).map(|(o, _)| o.as_str())
    }

    /// A monitor appeared: give it a desk. It gets back the lowest desk it
    /// owned before (reconnect), else the unplaced desk 1 (first boot), else
    /// the lowest free number.
    pub fn output_added(&mut self, output: &str) {
        if self.shown_on(output).is_some() { return; }
        let pick = self.list.iter()
            .filter(|w| !self.is_shown(w.id))
            .find(|w| w.output == output || w.output.is_empty())
            .map(|w| w.id)
            .unwrap_or_else(|| (1..).find(|id| self.get(*id).is_none()).unwrap());
        let _ = self.get_mut(pick);
        if let Some(w) = self.get_mut_existing(pick) { w.output = output.to_string(); }
        self.shown.push((output.to_string(), pick));
        // first monitor (or the focused one was unplaced): focus it
        if self.get(self.active).map(|w| w.output.is_empty()).unwrap_or(true)
            || !self.is_shown(self.active)
        {
            self.active = pick;
        }
    }

    /// A monitor went away: its desk goes off screen (it keeps remembering
    /// the monitor, so a reconnect brings it straight back). Returns that
    /// desk's windows, which the caller unmaps.
    pub fn output_removed(&mut self, output: &str) -> Vec<Window> {
        let Some(i) = self.shown.iter().position(|(o, _)| o == output) else { return Vec::new() };
        let (_, id) = self.shown.remove(i);
        let hidden = self.get(id).map(|w| w.windows()).unwrap_or_default();
        if self.active == id {
            if let Some((_, next)) = self.shown.first() { self.active = *next; }
        }
        hidden
    }

    /// Focus a monitor (the pointer moved onto it). True if focus changed.
    pub fn focus_output(&mut self, output: &str) -> bool {
        match self.shown_on(output) {
            Some(id) if id != self.active => { self.active = id; true }
            _ => false,
        }
    }

    /// Switch the focused monitor to desk `id` (creating it if needed).
    /// Returns the windows that must be hidden (the desk this monitor was
    /// showing) — caller unmaps them. A desk already on screen on ANOTHER
    /// monitor isn't stolen: focus just moves to that monitor (the caller
    /// warps the pointer there).
    pub fn switch_to(&mut self, id: u32) -> Vec<Window> {
        if id == self.active { return Vec::new(); }
        if self.is_shown(id) {
            self.previous = self.active;
            self.active = id;
            return Vec::new();
        }
        let out = self.focused_output().to_string();
        let hidden = self.active_ref().windows();
        let _ = self.get_mut(id);   // ensure it exists
        if let Some(w) = self.get_mut_existing(id) { w.output = out.clone(); }
        match self.shown.iter_mut().find(|(o, _)| *o == out) {
            Some(slot) => slot.1 = id,
            None => self.shown.push((out, id)),
        }
        self.previous = self.active;
        self.active = id;
        self.prune_empty();
        hidden
    }

    /// Adjacent workspace ON THE FOCUSED MONITOR (workspace-next/prev, the
    /// 3-finger swipe). `forward` may step past the last one to spawn a fresh
    /// desk, GNOME-style, but only if the current one isn't already empty.
    pub fn adjacent_id(&self, forward: bool) -> Option<u32> {
        let out = self.focused_output();
        let ids: Vec<u32> = self.list.iter()
            .filter(|w| w.output == out || w.id == self.active)
            .map(|w| w.id).collect();
        let pos = ids.iter().position(|&i| i == self.active)?;
        if forward {
            if pos + 1 < ids.len() { return Some(ids[pos + 1]); }
            if !self.active_ref().is_empty() {
                // a fresh number nobody (on any monitor) is using
                return (self.active + 1..).find(|id| self.get(*id).is_none());
            }
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
        for ws in &mut self.list { ws.prune_dead(); ws.prune_stacks(); }
        self.scratchpad.retain(|w| w.alive());
        self.prune_empty();
    }

    /// Drop empty workspaces that aren't on any monitor (dynamic policy).
    /// The focused one and every desk on screen always survive.
    fn prune_empty(&mut self) {
        let active = self.active;
        let shown: Vec<u32> = self.shown.iter().map(|(_, id)| *id).collect();
        self.list.retain(|w| w.id == active || shown.contains(&w.id) || !w.is_empty());
    }

    /// Snapshot for IPC / the bar: (id, window-count, output, on-screen)
    /// plus the focused id.
    pub fn snapshot(&self) -> (u32, Vec<(u32, usize, String, bool)>) {
        let mut out: Vec<(u32, usize, String, bool)> = self.list.iter()
            .map(|ws| (ws.id, ws.windows().len(), ws.output.clone(), self.is_shown(ws.id)))
            .collect();
        out.sort_by_key(|(id, ..)| *id);
        (self.active, out)
    }

    /// Windows on every desk that's on screen.
    pub fn visible_windows(&self) -> Vec<Window> {
        self.shown.iter()
            .filter_map(|(_, id)| self.get(*id))
            .flat_map(|w| w.windows())
            .collect()
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
