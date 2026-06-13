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

/// Bundled defaults — used when no user config is present. Targets the
/// packages that vendiOS ships in BASE_PKGS so every binding works out of
/// the box on a fresh install (alacritty, firefox, wofi, grim, slurp, wl-copy,
/// playerctl, brightnessctl, pipewire/wpctl).
const DEFAULT_CONFIG: &str = r#"
binds {
    // ── apps ───────────────────────────────────────────────────
    // Alacritty is the preferred terminal; fall back to foot on systems that
    // shipped before the alacritty package was added.
    bind "super+return"        "spawn sh -c 'alacritty || foot'"
    bind "super+b"             "spawn firefox"
    // Launchers: the dispatcher picks the right one for the bar in use
    // (quickshell spotlight/dashboard on pro, vendi-menu on classic).
    bind "super+d"             "spawn vendi-launcher dash"
    bind "super+space"         "spawn vendi-launcher"
    bind "super+alt+space"     "spawn vendi-launcher actions"

    // ── window management ──────────────────────────────────────
    bind "super+q"             "close"
    bind "super+j"             "focus-next"
    bind "super+tab"           "focus-next"
    bind "super+shift+tab"     "focus-prev"
    bind "super+k"             "spawn vendi-menu keys"
    bind "super+escape"        "spawn vendi-ctl lock"
    bind "super+left"          "focus-left"
    bind "super+right"         "focus-right"
    bind "super+up"            "focus-up"
    bind "super+down"          "focus-down"
    bind "super+shift+left"    "move-left"
    bind "super+shift+right"   "move-right"
    bind "super+shift+up"      "move-up"
    bind "super+shift+down"    "move-down"
    bind "super+ctrl+left"     "resize-left"
    bind "super+ctrl+right"    "resize-right"
    bind "super+ctrl+up"       "resize-up"
    bind "super+ctrl+down"     "resize-down"
    bind "super+h"             "split-horizontal"
    bind "super+v"             "split-vertical"
    bind "super+f"             "fullscreen"
    bind "super+o"             "overview"
    bind "super+shift+space"   "toggle-floating"
    bind "super+shift+escape"  "quit"

    // ── workspaces (dynamic — created on demand) ───────────────
    bind "super+1"             "workspace 1"
    bind "super+2"             "workspace 2"
    bind "super+3"             "workspace 3"
    bind "super+4"             "workspace 4"
    bind "super+5"             "workspace 5"
    bind "super+6"             "workspace 6"
    bind "super+7"             "workspace 7"
    bind "super+8"             "workspace 8"
    bind "super+9"             "workspace 9"
    bind "super+shift+1"       "move-to-workspace 1"
    bind "super+shift+2"       "move-to-workspace 2"
    bind "super+shift+3"       "move-to-workspace 3"
    bind "super+shift+4"       "move-to-workspace 4"
    bind "super+shift+5"       "move-to-workspace 5"
    bind "super+shift+6"       "move-to-workspace 6"
    bind "super+shift+7"       "move-to-workspace 7"
    bind "super+shift+8"       "move-to-workspace 8"
    bind "super+shift+9"       "move-to-workspace 9"

    // ── screenshots ────────────────────────────────────────────
    bind "print"               "spawn sh -c 'mkdir -p ~/Pictures && grim ~/Pictures/screenshot-$(date +%s).png'"
    bind "super+shift+s"       "spawn sh -c 'grim -g \"$(slurp)\" - | wl-copy'"

    // ── media keys ─────────────────────────────────────────────
    bind "XF86AudioRaiseVolume" "spawn wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%+"
    bind "XF86AudioLowerVolume" "spawn wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%-"
    bind "XF86AudioMute"        "spawn wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"
    bind "XF86AudioMicMute"     "spawn wpctl set-mute @DEFAULT_AUDIO_SOURCE@ toggle"
    bind "XF86AudioPlay"        "spawn playerctl play-pause"
    bind "XF86AudioNext"        "spawn playerctl next"
    bind "XF86AudioPrev"        "spawn playerctl previous"

    // ── brightness ─────────────────────────────────────────────
    bind "XF86MonBrightnessUp"   "spawn brightnessctl set 5%+"
    bind "XF86MonBrightnessDown" "spawn brightnessctl set 5%-"
}
"#;

// ── KDL schema ────────────────────────────────────────────────────────────────

#[derive(knus::Decode, Debug)]
pub struct Document {
    #[knus(child)]
    pub binds: Option<BindsBlock>,
    #[knus(child)]
    pub theme: Option<ThemeBlock>,
}

#[derive(knus::Decode, Debug)]
pub struct ThemeBlock {
    /// Border color of the focused window, "#rrggbb".
    #[knus(child, unwrap(argument))]
    pub accent:     Option<String>,
    /// Border color of unfocused windows.
    #[knus(child, unwrap(argument))]
    pub inactive:   Option<String>,
    /// Desktop clear color (visible until the wallpaper covers it).
    #[knus(child, unwrap(argument))]
    pub background: Option<String>,
    /// Window corner radius, logical px.
    #[knus(child, unwrap(argument))]
    pub radius:     Option<i64>,
    /// Border thickness, logical px.
    #[knus(child, unwrap(argument))]
    pub border:     Option<i64>,
    /// Gap between tiles, logical px.
    #[knus(child, unwrap(argument))]
    pub gap:        Option<i64>,
    /// Gap at the screen edges, logical px.
    #[knus(child, unwrap(argument))]
    pub margin:     Option<i64>,
    /// Wallpaper image path (png/jpg). Falls back to the built-in gradient.
    #[knus(child, unwrap(argument))]
    pub wallpaper:  Option<String>,
    /// Frosted-glass blur behind overlay surfaces (vendi-menu). On by
    /// default; turn off on GPUs where the extra passes hurt.
    #[knus(child, unwrap(argument))]
    pub blur:       Option<bool>,
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

/// Resolved theme values used by the renderer + layout.
#[derive(Debug, Clone)]
pub struct Theme {
    pub accent:     [f32; 4],
    pub inactive:   [f32; 4],
    pub background: [f32; 4],
    pub radius:     f32,
    pub border:     i32,
    pub gap:        i32,
    pub margin:     i32,
    pub wallpaper:  Option<String>,
    pub blur:       bool,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            accent:     hex_color("#cba6f7").unwrap(),   // Mauve
            inactive:   hex_color("#45475a").unwrap(),   // Surface1
            background: hex_color("#1e1e2e").unwrap(),   // Base
            radius:     12.0,
            border:     2,
            gap:        10,
            margin:     14,
            wallpaper:  None,
            blur:       true,
        }
    }
}

pub struct Config {
    pub keybinds: HashMap<Chord, Action>,
    /// Human-readable bind list in config order, user overrides applied.
    /// Served over IPC (`list-binds`) for the Super+K keybinds menu.
    pub keybinds_pretty: Vec<(String, String)>,
    pub theme:    Theme,
}

impl Config {
    /// Defaults always load first; a user file overlays them. A config that
    /// only sets a theme block keeps every default bind, and a user bind on
    /// an already-bound chord replaces the default action.
    pub fn load() -> Result<Self> {
        let default_doc: Document = knus::parse("default.kdl", DEFAULT_CONFIG)
            .map_err(|e| anyhow::anyhow!("parse built-in config: {e}"))?;
        let mut user_doc: Option<Document> = match read_user_config()? {
            Some(text) => {
                tracing::info!("overlaying user config");
                Some(knus::parse("vendiwm.kdl", &text)
                    .map_err(|e| anyhow::anyhow!("parse vendiwm.kdl: {e}"))?)
            }
            None => None,
        };

        // (chord, pretty chord, pretty action, action) in config order. A user
        // bind on an existing chord replaces that slot so the pretty list
        // shows the override, not both.
        let mut entries: Vec<(Chord, String, String, Action)> = Vec::new();
        let default_binds = default_doc.binds.map(|b| b.entries).unwrap_or_default();
        let user_binds = user_doc.as_mut()
            .and_then(|d| d.binds.take())
            .map(|b| b.entries)
            .unwrap_or_default();
        for entry in default_binds.into_iter().chain(user_binds) {
            let chord = parse_chord(&entry.chord)
                .with_context(|| format!("parse chord {:?}", entry.chord))?;
            let action = parse_action(&entry.action)
                .with_context(|| format!("parse action {:?}", entry.action))?;
            match entries.iter_mut().find(|(c, ..)| *c == chord) {
                Some(slot) => *slot = (chord, entry.chord, entry.action, action),
                None       => entries.push((chord, entry.chord, entry.action, action)),
            }
        }

        let mut keybinds = HashMap::new();
        let mut keybinds_pretty = Vec::new();
        for (chord, pretty_chord, pretty_action, action) in entries {
            keybinds.insert(chord, action);
            keybinds_pretty.push((pretty_chord, pretty_action));
        }

        let mut theme = Theme::default();
        if let Some(t) = user_doc.and_then(|d| d.theme) {
            if let Some(c) = t.accent.as_deref().and_then(hex_color)     { theme.accent = c; }
            if let Some(c) = t.inactive.as_deref().and_then(hex_color)   { theme.inactive = c; }
            if let Some(c) = t.background.as_deref().and_then(hex_color) { theme.background = c; }
            if let Some(v) = t.radius  { theme.radius = v as f32; }
            if let Some(v) = t.border  { theme.border = v as i32; }
            if let Some(v) = t.gap     { theme.gap = v as i32; }
            if let Some(v) = t.margin  { theme.margin = v as i32; }
            if t.wallpaper.is_some()   { theme.wallpaper = t.wallpaper; }
            if let Some(v) = t.blur    { theme.blur = v; }
        }

        // Runtime wallpaper switches (vendi-ctl wallpaper / the bar's picker)
        // persist to ~/.config/vendi/wallpaper — the strongest override.
        if let Some(p) = read_wallpaper_override() {
            theme.wallpaper = Some(p);
        }

        Ok(Self { keybinds, keybinds_pretty, theme })
    }
}

/// "#rrggbb" or "#rrggbbaa" → premultiplied-friendly [r, g, b, a] floats.
fn hex_color(s: &str) -> Option<[f32; 4]> {
    let hex = s.trim().strip_prefix('#')?;
    if hex.len() != 6 && hex.len() != 8 { return None; }
    let p = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok().map(|v| v as f32 / 255.0);
    Some([p(0)?, p(2)?, p(4)?, if hex.len() == 8 { p(6)? } else { 1.0 }])
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

/// ~/.config/vendi/wallpaper: one line, the path of the last wallpaper set
/// at runtime. Missing/empty/stale paths are ignored.
fn read_wallpaper_override() -> Option<String> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    let text = std::fs::read_to_string(base.join("vendi/wallpaper")).ok()?;
    let p = text.trim();
    if !p.is_empty() && std::path::Path::new(p).is_file() {
        Some(p.to_string())
    } else {
        None
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
    use crate::input::Dir;
    Ok(match verb {
        "spawn"             => Action::Spawn(rest.to_string()),
        "close"             => Action::Close,
        "focus-next"        => Action::FocusNext,
        "focus-prev"        => Action::FocusPrev,
        "focus-left"        => Action::FocusDir(Dir::Left),
        "focus-right"       => Action::FocusDir(Dir::Right),
        "focus-up"          => Action::FocusDir(Dir::Up),
        "focus-down"        => Action::FocusDir(Dir::Down),
        "move-left"         => Action::MoveDir(Dir::Left),
        "move-right"        => Action::MoveDir(Dir::Right),
        "move-up"           => Action::MoveDir(Dir::Up),
        "move-down"         => Action::MoveDir(Dir::Down),
        "resize-left"       => Action::ResizeDir(Dir::Left),
        "resize-right"      => Action::ResizeDir(Dir::Right),
        "resize-up"         => Action::ResizeDir(Dir::Up),
        "resize-down"       => Action::ResizeDir(Dir::Down),
        "split-horizontal"  => Action::SetNextSplit(Direction::Horizontal),
        "split-vertical"    => Action::SetNextSplit(Direction::Vertical),
        "workspace"         => Action::Workspace(rest.parse().context("workspace number")?),
        "move-to-workspace" => Action::MoveToWorkspace(rest.parse().context("workspace number")?),
        "toggle-floating"   => Action::ToggleFloating,
        "fullscreen"        => Action::ToggleFullscreen,
        "overview"          => Action::ToggleOverview,
        "lock"              => Action::Lock,
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
