// KDL config loader for vendiwm.
//
// Path: $XDG_CONFIG_HOME/vendi/vendiwm.kdl  (default ~/.config/vendi/vendiwm.kdl)
//
// If the file doesn't exist, defaults compiled into this binary are used.
// Hot-reload via filesystem watcher is a follow-up — config currently loads
// at startup only.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use smithay::input::keyboard::xkb;

use crate::input::Action;
use crate::layout::Direction;

/// Bundled defaults — used when no user config is present.
const DEFAULT_CONFIG: &str = r#"
binds {
    bind "super+return"        "spawn alacritty"
    bind "super+d"             "spawn vendi-menu"
    bind "super+q"             "close"
    bind "super+j"             "focus-next"
    bind "super+k"             "focus-prev"
    bind "super+h"             "split-horizontal"
    bind "super+v"             "split-vertical"
    bind "super+shift+escape"  "quit"
}
"#;

// ── KDL schema ────────────────────────────────────────────────────────────────

#[derive(knus::Decode, Debug)]
pub struct Document {
    #[knus(child)]
    pub binds: Option<BindsBlock>,
}

#[derive(knus::Decode, Debug)]
pub struct BindsBlock {
    #[knus(children(name = "bind"))]
    pub entries: Vec<BindEntry>,
}

#[derive(knus::Decode, Debug)]
pub struct BindEntry {
    #[knus(argument)]
    pub chord:  String,
    #[knus(argument)]
    pub action: String,
}

// ── runtime config ────────────────────────────────────────────────────────────

/// Compiled keybind: (logo, ctrl, alt, shift, keysym) → action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    pub logo:  bool,
    pub ctrl:  bool,
    pub alt:   bool,
    pub shift: bool,
    pub key:   u32,   // xkb keysym
}

pub struct Config {
    pub keybinds: HashMap<Chord, Action>,
}

impl Config {
    /// Read user config or fall back to defaults.
    pub fn load() -> Result<Self> {
        let kdl_text = match read_user_config()? {
            Some(text) => {
                tracing::info!("loaded user config");
                text
            }
            None => {
                tracing::info!("using bundled default config");
                DEFAULT_CONFIG.to_string()
            }
        };
        let doc: Document = knus::parse("vendiwm.kdl", &kdl_text)
            .map_err(|e| anyhow::anyhow!("parse vendiwm.kdl: {e}"))?;

        let mut keybinds = HashMap::new();
        if let Some(binds) = doc.binds {
            for entry in binds.entries {
                let chord = parse_chord(&entry.chord)
                    .with_context(|| format!("parse chord {:?}", entry.chord))?;
                let action = parse_action(&entry.action)
                    .with_context(|| format!("parse action {:?}", entry.action))?;
                keybinds.insert(chord, action);
            }
        }
        Ok(Self { keybinds })
    }
}

fn read_user_config() -> Result<Option<String>> {
    let mut path = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(d) => PathBuf::from(d),
        None    => {
            let home = std::env::var_os("HOME")
                .ok_or_else(|| anyhow::anyhow!("HOME not set"))?;
            PathBuf::from(home).join(".config")
        }
    };
    path.push("vendi");
    path.push("vendiwm.kdl");

    match std::fs::read_to_string(&path) {
        Ok(text)  => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e)   => Err(anyhow::Error::new(e).context(format!("read {}", path.display()))),
    }
}

// ── chord parsing ─────────────────────────────────────────────────────────────

fn parse_chord(s: &str) -> Result<Chord> {
    let mut c = Chord { logo: false, ctrl: false, alt: false, shift: false, key: 0 };
    let mut parts: Vec<&str> = s.split('+').map(|p| p.trim()).collect();
    let key_name = parts.pop()
        .ok_or_else(|| anyhow::anyhow!("empty chord"))?
        .to_string();
    for m in parts {
        match m.to_ascii_lowercase().as_str() {
            "super" | "logo" | "mod" | "win" => c.logo  = true,
            "ctrl"  | "control"              => c.ctrl  = true,
            "alt"   | "mod1"                 => c.alt   = true,
            "shift"                          => c.shift = true,
            other => anyhow::bail!("unknown modifier {other:?}"),
        }
    }
    c.key = xkb::keysym_from_name(&key_name, xkb::KEYSYM_CASE_INSENSITIVE).raw();
    if c.key == xkb::keysyms::KEY_NoSymbol {
        anyhow::bail!("unknown key name {key_name:?}");
    }
    Ok(c)
}

// ── action parsing ────────────────────────────────────────────────────────────

fn parse_action(s: &str) -> Result<Action> {
    let mut parts = s.splitn(2, char::is_whitespace);
    let verb = parts.next().unwrap_or("").trim();
    let rest = parts.next().map(str::trim).unwrap_or("");
    Ok(match verb {
        "spawn"             => Action::Spawn(rest.to_string()),
        "close"             => Action::Close,
        "focus-next"        => Action::FocusNext,
        "focus-prev"        => Action::FocusPrev,
        "split-horizontal"  => Action::SetNextSplit(Direction::Horizontal),
        "split-vertical"    => Action::SetNextSplit(Direction::Vertical),
        "quit"              => Action::Quit,
        other => anyhow::bail!("unknown action verb {other:?}"),
    })
}

/// Build a `Chord` from runtime modifier state + keysym for lookup.
pub fn chord_from(mods: &smithay::input::keyboard::ModifiersState, sym: u32) -> Chord {
    Chord {
        logo:  mods.logo,
        ctrl:  mods.ctrl,
        alt:   mods.alt,
        shift: mods.shift,
        key:   sym,
    }
}
