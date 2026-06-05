// Keybinding handler.
//
// Recognized inside `keyboard.input(...)` callbacks in the winit/udev event
// loops. Returns Some(Action) if the key combo is a vendiwm shortcut and the
// client should NOT see the event; None to forward to the focused client.
//
// The keymap is currently hardcoded; KDL config + hot-reload lands later.

use smithay::input::keyboard::{ModifiersState, xkb};
use smithay::backend::input::KeyState;

use crate::layout::Direction;

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
    /// Set the direction for the NEXT window insert (consumed on use).
    SetNextSplit(Direction),
    /// Quit the compositor.
    Quit,
}

/// Resolve a keypress to an Action. Returns None for keys we don't bind.
pub fn handle(keysym: u32, state: KeyState, mods: &ModifiersState) -> Option<Action> {
    // Only intercept on press.
    if state != KeyState::Pressed { return None; }
    // Only intercept Super combos. Anything else goes to the focused client.
    if !mods.logo { return None; }

    match keysym {
        // Spawn — Super+Return launches a terminal.
        k if k == xkb::keysyms::KEY_Return  => Some(Action::Spawn("alacritty".into())),
        // Close focused window.
        k if k == xkb::keysyms::KEY_q       => Some(Action::Close),
        // Focus cycling.
        k if k == xkb::keysyms::KEY_j       => Some(Action::FocusNext),
        k if k == xkb::keysyms::KEY_k       => Some(Action::FocusPrev),
        // Next-split direction override.
        k if k == xkb::keysyms::KEY_h       => Some(Action::SetNextSplit(Direction::Horizontal)),
        k if k == xkb::keysyms::KEY_v       => Some(Action::SetNextSplit(Direction::Vertical)),
        // Hard quit (development convenience — Super+Shift+E in real use).
        k if k == xkb::keysyms::KEY_Escape && mods.shift => Some(Action::Quit),
        _ => None,
    }
}
