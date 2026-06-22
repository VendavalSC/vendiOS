// i3-style tiling tree.
//
// Each leaf holds a Window. Each split holds N children with a direction
// (horizontal = left/right neighbors, vertical = top/bottom) and per-child
// ratios that sum to 1.0.
//
// Insertion policy: a new window splits the focused leaf perpendicular to
// the focused leaf's parent (so opening windows produces balanced grids).
// Explicit Super+H / Super+V overrides the default for the next insert.

use smithay::desktop::Window;
use smithay::utils::{IsAlive, Logical, Rectangle, Size};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Horizontal,  // children laid out left → right
    Vertical,    // children laid out top  → bottom
}

impl Direction {
    pub fn perpendicular(self) -> Self {
        match self {
            Direction::Horizontal => Direction::Vertical,
            Direction::Vertical   => Direction::Horizontal,
        }
    }
}

/// How a workspace arranges its tiled windows. The window set + focus order
/// always live in the BSP `Tree`; the non-tiling modes just place that same
/// set differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    /// i3-style binary split tree (the default).
    Tiling,
    /// One master pane on the left, the rest stacked on the right (dwm-style).
    Master,
    /// Every window fills the screen; only the focused one shows on top.
    Monocle,
    /// Each window halves the remaining area, alternating axis — windows
    /// dwindle toward a corner (hyprland-style).
    Dwindle,
    /// Like dwindle, but the open area rotates so windows spiral inward
    /// (Fibonacci).
    Spiral,
}

impl LayoutMode {
    pub fn next(self) -> Self {
        match self {
            LayoutMode::Tiling  => LayoutMode::Master,
            LayoutMode::Master  => LayoutMode::Monocle,
            LayoutMode::Monocle => LayoutMode::Dwindle,
            LayoutMode::Dwindle => LayoutMode::Spiral,
            LayoutMode::Spiral  => LayoutMode::Tiling,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            LayoutMode::Tiling  => "Tiling",
            LayoutMode::Master  => "Master-stack",
            LayoutMode::Monocle => "Monocle",
            LayoutMode::Dwindle => "Dwindle",
            LayoutMode::Spiral  => "Spiral",
        }
    }
}

#[derive(Debug)]
enum Node {
    Leaf(Window),
    Split { dir: Direction, ratios: Vec<f32>, children: Vec<Node> },
}

pub struct Tree {
    root: Option<Node>,
    // Path from root to the focused leaf — each step is a child index.
    focus_path: Vec<usize>,
    // Direction override for the next insert (set by Super+H/V keybinds).
    pub next_split_override: Option<Direction>,
}

impl Tree {
    pub fn new() -> Self {
        Self { root: None, focus_path: Vec::new(), next_split_override: None }
    }

    pub fn is_empty(&self) -> bool { self.root.is_none() }

    /// Insert a window. Always becomes the focused leaf.
    pub fn insert(&mut self, window: Window) {
        if self.root.is_none() {
            self.root = Some(Node::Leaf(window));
            self.focus_path.clear();
            return;
        }

        // Direction to split the focused leaf with:
        // 1. Explicit override (Super+H/V), consumed on use.
        // 2. Otherwise perpendicular to the focused leaf's parent.
        // 3. Single-window root has no parent — default Horizontal.
        let dir = self.next_split_override
            .take()
            .unwrap_or_else(|| {
                self.parent_direction()
                    .map(|d| d.perpendicular())
                    .unwrap_or(Direction::Horizontal)
            });

        // Replace the focused leaf with a Split([old_leaf, new_leaf]).
        let path = self.focus_path.clone();
        let root = self.root.take().unwrap();
        self.root = Some(insert_at(root, &path, window, dir));
        // Focus the new leaf (second child of the new Split).
        self.focus_path.push(1);
    }

    fn parent_direction(&self) -> Option<Direction> {
        if self.focus_path.is_empty() { return None; }
        let mut node = self.root.as_ref()?;
        for &idx in &self.focus_path[..self.focus_path.len() - 1] {
            if let Node::Split { children, .. } = node { node = &children[idx]; }
            else { return None; }
        }
        if let Node::Split { dir, .. } = node { Some(*dir) } else { None }
    }

    /// Remove the given window. Promotes a singleton split's remaining child
    /// back up so the tree stays tight.
    pub fn remove(&mut self, window: &Window) {
        let root = match self.root.take() { Some(r) => r, None => return };
        let (root, removed) = Self::remove_node(root, window);
        self.root = root;
        if removed {
            // Move focus to a sensible neighbor — for now, root or empty.
            self.focus_path.clear();
            // If root is still a Split, focus first leaf path.
            if let Some(ref r) = self.root {
                self.focus_path = first_leaf_path(r);
            }
        }
    }

    fn remove_node(node: Node, target: &Window) -> (Option<Node>, bool) {
        match node {
            Node::Leaf(w) => {
                if &w == target { (None, true) } else { (Some(Node::Leaf(w)), false) }
            }
            Node::Split { dir, mut ratios, children } => {
                let mut new_children = Vec::with_capacity(children.len());
                let mut new_ratios   = Vec::with_capacity(ratios.len());
                let mut hit          = false;
                for (i, c) in children.into_iter().enumerate() {
                    let (child, removed) = Self::remove_node(c, target);
                    if let Some(child) = child {
                        new_children.push(child);
                        new_ratios.push(ratios[i]);
                    }
                    hit |= removed;
                }
                match new_children.len() {
                    0 => (None, hit),
                    1 => (Some(new_children.into_iter().next().unwrap()), hit),
                    _ => {
                        // Renormalize ratios.
                        let sum: f32 = new_ratios.iter().sum();
                        if sum > 0.0 { for r in &mut new_ratios { *r /= sum; } }
                        ratios = new_ratios;
                        (Some(Node::Split { dir, ratios, children: new_children }), hit)
                    }
                }
            }
        }
    }

    /// Focus the next leaf in a depth-first traversal (Tab order).
    pub fn focus_next(&mut self) { self.focus_step(true);  }
    pub fn focus_prev(&mut self) { self.focus_step(false); }

    fn focus_step(&mut self, forward: bool) {
        let Some(root) = self.root.as_ref() else { return };
        let mut leaves: Vec<Vec<usize>> = Vec::new();
        collect_leaf_paths(root, Vec::new(), &mut leaves);
        if leaves.is_empty() { return; }
        let idx = leaves.iter().position(|p| p == &self.focus_path).unwrap_or(0);
        let next = if forward { (idx + 1) % leaves.len() } else { (idx + leaves.len() - 1) % leaves.len() };
        self.focus_path = leaves[next].clone();
    }

    /// Drop any leaves whose Window is no longer alive (client closed).
    pub fn prune_dead(&mut self) {
        let root = match self.root.take() { Some(r) => r, None => return };
        self.root = prune_dead_node(root);
        // Reset focus if the path no longer points at a live leaf.
        if let Some(ref r) = self.root {
            if !path_valid(r, &self.focus_path) {
                self.focus_path = first_leaf_path(r);
            }
        } else {
            self.focus_path.clear();
        }
    }

    pub fn focused(&self) -> Option<&Window> {
        let mut node = self.root.as_ref()?;
        for &idx in &self.focus_path {
            if let Node::Split { children, .. } = node { node = &children[idx]; }
            else { return None; }
        }
        if let Node::Leaf(w) = node { Some(w) } else { None }
    }

    /// Compute a Window → screen rectangle mapping for the whole tree.
    pub fn layout(&self, viewport: Rectangle<i32, Logical>) -> Vec<(Window, Rectangle<i32, Logical>)> {
        let mut out = Vec::new();
        if let Some(root) = self.root.as_ref() {
            layout_node(root, viewport, &mut out);
        }
        out
    }

    /// Place windows for the given layout mode. Tiling defers to the BSP tree;
    /// Master/Monocle reuse the tree's window set + order.
    pub fn placements(&self, vp: Rectangle<i32, Logical>, mode: LayoutMode)
        -> Vec<(Window, Rectangle<i32, Logical>)>
    {
        match mode {
            LayoutMode::Tiling  => self.layout(vp),
            LayoutMode::Monocle => self.windows().into_iter().map(|w| (w, vp)).collect(),
            LayoutMode::Master  => master_stack(&self.windows(), vp),
            LayoutMode::Dwindle => dwindle(&self.windows(), vp),
            LayoutMode::Spiral  => spiral(&self.windows(), vp),
        }
    }

    /// All windows in DFS (visual) order.
    pub fn windows(&self) -> Vec<Window> {
        let mut out = Vec::new();
        if let Some(root) = self.root.as_ref() {
            collect_windows(root, &mut out);
        }
        out
    }

    pub fn contains(&self, window: &Window) -> bool {
        self.windows().iter().any(|w| w == window)
    }

    /// Point focus at the leaf holding `window`. Returns false if absent.
    pub fn focus_window(&mut self, window: &Window) -> bool {
        let Some(root) = self.root.as_ref() else { return false };
        let mut leaves: Vec<(Vec<usize>, Window)> = Vec::new();
        collect_leaves(root, Vec::new(), &mut leaves);
        if let Some((path, _)) = leaves.into_iter().find(|(_, w)| w == window) {
            self.focus_path = path;
            true
        } else {
            false
        }
    }

    /// Swap the tree positions of two windows (used for Super+Shift+arrow
    /// "move window" — geometry stays a clean tiling, only occupants move).
    pub fn swap_windows(&mut self, a: &Window, b: &Window) {
        if let Some(root) = self.root.as_mut() {
            swap_in_node(root, a, b);
        }
    }

    /// Grow (+delta) or shrink (-delta) the focused window along `axis` by
    /// adjusting the ratio split of the nearest ancestor running that axis.
    pub fn resize_focused(&mut self, axis: Direction, delta: f32) {
        // Find the deepest ancestor split along the focus path whose
        // direction matches `axis` — that's the split the user perceives
        // as "my window's edge in that direction".
        let Some(root) = self.root.as_ref() else { return };
        let mut depth_match: Option<usize> = None;
        let mut node = root;
        for (depth, &idx) in self.focus_path.iter().enumerate() {
            if let Node::Split { dir, children, .. } = node {
                if *dir == axis { depth_match = Some(depth); }
                node = &children[idx];
            } else {
                break;
            }
        }
        let Some(depth) = depth_match else {
            tracing::debug!(?axis, "resize: no split along the focus path runs this axis");
            return;
        };

        // Re-walk mutably and apply the ratio change at that depth.
        let mut node = self.root.as_mut().unwrap();
        for &idx in &self.focus_path[..depth] {
            if let Node::Split { children, .. } = node { node = &mut children[idx]; }
            else { return; }
        }
        if let Node::Split { ratios, .. } = node {
            let i = self.focus_path[depth];
            // Trade ratio with the next sibling (or previous if we're last).
            let j = if i + 1 < ratios.len() { i + 1 } else { i.wrapping_sub(1) };
            if j >= ratios.len() || i == j { return; }
            // Arrow = the direction the shared edge moves. Growing ratio[i]
            // pushes the edge shared with the NEXT sibling; when we trade
            // with the PREVIOUS one (focused pane is last), the same arrow
            // must move that edge the opposite way.
            let delta = if j < i { -delta } else { delta };
            const MIN: f32 = 0.10;
            let d = delta.clamp(MIN - ratios[i], ratios[j] - MIN);
            ratios[i] += d;
            ratios[j] -= d;
        }
    }
}

impl Default for Tree {
    fn default() -> Self { Self::new() }
}

/// Take ownership of a node, walk to `path`, and replace the leaf there with
/// a fresh Split(`[old_leaf, new_leaf]`) in the given direction.
fn insert_at(node: Node, path: &[usize], new_window: Window, dir: Direction) -> Node {
    if path.is_empty() {
        // This IS the focused leaf — wrap it.
        return Node::Split {
            dir,
            ratios:   vec![0.5, 0.5],
            children: vec![node, Node::Leaf(new_window)],
        };
    }
    match node {
        Node::Leaf(_) => node,   // path was malformed — shouldn't happen
        Node::Split { dir: d, ratios, mut children } => {
            let i = path[0];
            let child = children.remove(i);
            let replaced = insert_at(child, &path[1..], new_window, dir);
            children.insert(i, replaced);
            Node::Split { dir: d, ratios, children }
        }
    }
}

/// dwm-style master-stack: first window fills ~60% on the left, the rest split
/// the right column evenly top→bottom. One window fills the viewport.
fn master_stack(ws: &[Window], vp: Rectangle<i32, Logical>) -> Vec<(Window, Rectangle<i32, Logical>)> {
    let n = ws.len();
    if n == 0 { return Vec::new(); }
    if n == 1 { return vec![(ws[0].clone(), vp)]; }
    let mw = (vp.size.w as f32 * 0.6).round() as i32;
    let mut out = vec![(ws[0].clone(),
        Rectangle::new(vp.loc, Size::from((mw, vp.size.h))))];
    let stack_n = (n - 1) as i32;
    let sx = vp.loc.x + mw;
    let sw = vp.size.w - mw;
    let sh = vp.size.h / stack_n;
    for (i, w) in ws[1..].iter().enumerate() {
        let y = vp.loc.y + i as i32 * sh;
        // last stack window soaks up the rounding remainder
        let h = if i as i32 == stack_n - 1 { vp.loc.y + vp.size.h - y } else { sh };
        out.push((w.clone(), Rectangle::new((sx, y).into(), Size::from((sw, h)))));
    }
    out
}

/// Dwindle: each window takes half of the remaining area (alternating the split
/// axis along whichever side is longer); the rest dwindle into the other half.
/// The last window fills whatever is left.
fn dwindle(ws: &[Window], vp: Rectangle<i32, Logical>) -> Vec<(Window, Rectangle<i32, Logical>)> {
    let mut out = Vec::new();
    let n = ws.len();
    if n == 0 { return out; }
    let mut rect = vp;
    let mut horiz = rect.size.w >= rect.size.h;
    for (i, w) in ws.iter().enumerate() {
        if i == n - 1 {
            out.push((w.clone(), rect));
            break;
        }
        if horiz {
            let half = rect.size.w / 2;
            out.push((w.clone(), Rectangle::new(rect.loc, Size::from((half, rect.size.h)))));
            rect = Rectangle::new((rect.loc.x + half, rect.loc.y).into(),
                                  Size::from((rect.size.w - half, rect.size.h)));
        } else {
            let half = rect.size.h / 2;
            out.push((w.clone(), Rectangle::new(rect.loc, Size::from((rect.size.w, half)))));
            rect = Rectangle::new((rect.loc.x, rect.loc.y + half).into(),
                                  Size::from((rect.size.w, rect.size.h - half)));
        }
        horiz = !horiz;
    }
    out
}

/// Spiral (Fibonacci): like dwindle, but the side the window takes alternates,
/// so the open area rotates and windows spiral inward.
fn spiral(ws: &[Window], vp: Rectangle<i32, Logical>) -> Vec<(Window, Rectangle<i32, Logical>)> {
    let mut out = Vec::new();
    let n = ws.len();
    if n == 0 { return out; }
    let mut rect = vp;
    let mut horiz = rect.size.w >= rect.size.h;
    let mut second = false; // which half the window takes — flips each step
    for (i, w) in ws.iter().enumerate() {
        if i == n - 1 {
            out.push((w.clone(), rect));
            break;
        }
        if horiz {
            let half = rect.size.w / 2;
            let (win, rest) = if second {
                (Rectangle::new((rect.loc.x + half, rect.loc.y).into(), Size::from((rect.size.w - half, rect.size.h))),
                 Rectangle::new(rect.loc, Size::from((half, rect.size.h))))
            } else {
                (Rectangle::new(rect.loc, Size::from((half, rect.size.h))),
                 Rectangle::new((rect.loc.x + half, rect.loc.y).into(), Size::from((rect.size.w - half, rect.size.h))))
            };
            out.push((w.clone(), win));
            rect = rest;
        } else {
            let half = rect.size.h / 2;
            let (win, rest) = if second {
                (Rectangle::new((rect.loc.x, rect.loc.y + half).into(), Size::from((rect.size.w, rect.size.h - half))),
                 Rectangle::new(rect.loc, Size::from((rect.size.w, half))))
            } else {
                (Rectangle::new(rect.loc, Size::from((rect.size.w, half))),
                 Rectangle::new((rect.loc.x, rect.loc.y + half).into(), Size::from((rect.size.w, rect.size.h - half))))
            };
            out.push((w.clone(), win));
            rect = rest;
        }
        horiz = !horiz;
        second = !second;
    }
    out
}

fn layout_node(node: &Node, rect: Rectangle<i32, Logical>, out: &mut Vec<(Window, Rectangle<i32, Logical>)>) {
    match node {
        Node::Leaf(w) => out.push((w.clone(), rect)),
        Node::Split { dir, ratios, children } => {
            let total = match dir { Direction::Horizontal => rect.size.w, Direction::Vertical => rect.size.h };
            let mut cursor = 0;
            for (i, child) in children.iter().enumerate() {
                let span = if i == children.len() - 1 {
                    total - cursor   // give last child whatever's left to avoid rounding gaps
                } else {
                    (total as f32 * ratios[i]).round() as i32
                };
                let sub = match dir {
                    Direction::Horizontal => Rectangle::new(
                        (rect.loc.x + cursor, rect.loc.y).into(),
                        Size::from((span, rect.size.h)),
                    ),
                    Direction::Vertical => Rectangle::new(
                        (rect.loc.x, rect.loc.y + cursor).into(),
                        Size::from((rect.size.w, span)),
                    ),
                };
                layout_node(child, sub, out);
                cursor += span;
            }
        }
    }
}

fn collect_windows(node: &Node, out: &mut Vec<Window>) {
    match node {
        Node::Leaf(w) => out.push(w.clone()),
        Node::Split { children, .. } => for c in children { collect_windows(c, out); },
    }
}

fn collect_leaves(node: &Node, path: Vec<usize>, out: &mut Vec<(Vec<usize>, Window)>) {
    match node {
        Node::Leaf(w) => out.push((path, w.clone())),
        Node::Split { children, .. } => {
            for (i, c) in children.iter().enumerate() {
                let mut p = path.clone();
                p.push(i);
                collect_leaves(c, p, out);
            }
        }
    }
}

fn swap_in_node(node: &mut Node, a: &Window, b: &Window) {
    match node {
        Node::Leaf(w) => {
            if w == a { *w = b.clone(); }
            else if w == b { *w = a.clone(); }
        }
        Node::Split { children, .. } => {
            for c in children { swap_in_node(c, a, b); }
        }
    }
}

fn collect_leaf_paths(node: &Node, path: Vec<usize>, out: &mut Vec<Vec<usize>>) {
    match node {
        Node::Leaf(_) => out.push(path),
        Node::Split { children, .. } => {
            for (i, c) in children.iter().enumerate() {
                let mut p = path.clone();
                p.push(i);
                collect_leaf_paths(c, p, out);
            }
        }
    }
}

fn prune_dead_node(node: Node) -> Option<Node> {
    match node {
        Node::Leaf(w) => if w.alive() { Some(Node::Leaf(w)) } else { None },
        Node::Split { dir, mut ratios, children } => {
            let mut kept_children = Vec::new();
            let mut kept_ratios   = Vec::new();
            for (i, c) in children.into_iter().enumerate() {
                if let Some(c) = prune_dead_node(c) {
                    kept_children.push(c);
                    kept_ratios.push(ratios[i]);
                }
            }
            match kept_children.len() {
                0 => None,
                1 => Some(kept_children.into_iter().next().unwrap()),
                _ => {
                    let sum: f32 = kept_ratios.iter().sum();
                    if sum > 0.0 { for r in &mut kept_ratios { *r /= sum; } }
                    ratios = kept_ratios;
                    Some(Node::Split { dir, ratios, children: kept_children })
                }
            }
        }
    }
}

fn path_valid(node: &Node, path: &[usize]) -> bool {
    if path.is_empty() { return matches!(node, Node::Leaf(_)); }
    match node {
        Node::Leaf(_) => false,
        Node::Split { children, .. } => {
            path[0] < children.len() && path_valid(&children[path[0]], &path[1..])
        }
    }
}

fn first_leaf_path(node: &Node) -> Vec<usize> {
    let mut p = Vec::new();
    let mut cur = node;
    while let Node::Split { children, .. } = cur {
        p.push(0);
        cur = &children[0];
    }
    p
}
