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
