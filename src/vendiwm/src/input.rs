// Keybinding handler.
//
// Recognized inside `keyboard.input(...)` callbacks in the winit/udev event
// loops. Returns Some(Action) if the key combo is a vendiwm shortcut and the
// client should NOT see the event; None to forward to the focused client.
//
// The keymap is currently hardcoded; KDL config + hot-reload lands later.

use smithay::input::keyboard::ModifiersState;
use smithay::backend::input::KeyState;

use crate::config::{Config, chord_from};
use crate::layout::Direction;

/// Screen-space direction for focus / move / resize actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir { Left, Right, Up, Down }

impl Dir {
    /// The split axis this direction operates on.
    pub fn axis(self) -> Direction {
        match self {
            Dir::Left | Dir::Right => Direction::Horizontal,
            Dir::Up   | Dir::Down  => Direction::Vertical,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Action {
    /// Spawn a child process from a command string.
    Spawn(String),
    /// Close the focused window.
    Close,
    /// Cycle focus to the next leaf in the layout tree.
    FocusNext,
    /// Cycle focus to the previous leaf.
    FocusPrev,
    /// Focus the nearest window in a screen direction.
    FocusDir(Dir),
    /// Swap the focused window with its neighbor in a screen direction.
    MoveDir(Dir),
    /// Grow/shrink the focused window's split toward a direction.
    ResizeDir(Dir),
    /// Set the direction for the NEXT window insert (consumed on use).
    SetNextSplit(Direction),
    /// Switch to workspace N (created on demand).
    Workspace(u32),
    /// Send the focused window to workspace N.
    MoveToWorkspace(u32),
    /// Toggle the focused window between tiled and floating.
    ToggleFloating,
    /// Toggle fullscreen on the focused window.
    ToggleFullscreen,
    /// Toggle the overview (exposé) window grid.
    ToggleOverview,
    Lock,
    /// Quit the compositor.
    Quit,
}

/// Resolve a keypress to an Action via the loaded config. Returns None for
/// keys not bound by the user (or only on release).
pub fn handle(config: &Config, keysym: u32, state: KeyState, mods: &ModifiersState) -> Option<Action> {
    if state != KeyState::Pressed { return None; }
    let chord = chord_from(mods, keysym);
    config.keybinds.get(&chord).cloned()
}

// Keep Direction in scope for the action-action ladder to silence unused-import
// warnings when reorganizing.
#[allow(dead_code)]
fn _force_direction_import(_: Direction) {}
