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
    // kitty is the default terminal (GPU, inline images, animated cursor
    // trail); fall back to alacritty then foot on systems that predate it.
    bind "super+return"        "spawn sh -c 'kitty || alacritty || foot'"
    bind "super+b"             "spawn firefox"
    bind "super+e"             "spawn nautilus"
    // Launchers: the dispatcher picks the right one for the bar in use
    // (quickshell spotlight/dashboard on pro, vendi-menu on classic).
    bind "super+d"             "spawn vendi-launcher dash"
    bind "super+space"         "spawn vendi-launcher"
    bind "super+alt+space"     "spawn vendi-launcher actions"
    bind "super+a"             "spawn vendi-launcher ai"

    // ── window management ──────────────────────────────────────
    bind "super+q"             "close"
    bind "super+shift+q"       "kill"
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
    bind "super+t"             "cycle-layout"
    bind "super+o"             "overview"
    bind "super+shift+o"       "cycle-opacity"
    bind "super+shift+b"       "toggle-blur"
    bind "super+shift+space"   "toggle-floating"
    bind "super+c"             "center-floating"
    bind "super+shift+r"       "reload-config"
    bind "super+shift+escape"  "quit"

    // workspace navigation
    bind "super+period"        "workspace-next"
    bind "super+comma"         "workspace-prev"
    bind "super+grave"         "workspace-last"

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
    // Print = full-screen shot to ~/Pictures, also copied to the clipboard, with
    // a toast so you know it fired. Super+Shift+S = pick a region to the clipboard.
    bind "print"               "spawn sh -c 'mkdir -p ~/Pictures; f=~/Pictures/screenshot-$(date +%Y%m%d-%H%M%S).png; grim $f && wl-copy < $f && notify-send -a Screenshot Screenshot \"Saved $f\"'"
    bind "super+shift+s"       "spawn sh -c 'grim -g \"$(slurp)\" - | wl-copy && notify-send -a Screenshot Screenshot \"Region copied to clipboard\"'"

    // ── media keys ─────────────────────────────────────────────
    bind "XF86AudioRaiseVolume" "spawn wpctl set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+"
    bind "XF86AudioLowerVolume" "spawn wpctl set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%-"
    bind "XF86AudioMute"        "spawn wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"
    bind "XF86AudioMicMute"     "spawn wpctl set-mute @DEFAULT_AUDIO_SOURCE@ toggle"
    bind "XF86AudioPlay"        "spawn playerctl play-pause"
    bind "XF86AudioNext"        "spawn playerctl next"
    bind "XF86AudioPrev"        "spawn playerctl previous"

    // ── brightness ─────────────────────────────────────────────
    bind "XF86MonBrightnessUp"   "spawn brightnessctl set 5%+"
    bind "XF86MonBrightnessDown" "spawn brightnessctl set 5%-"
}

// Idle behaviour (seconds; 0 disables each). Any keyboard / pointer / touch
// input resets the timers. The screen powers off via DPMS and wakes on input.
idle {
    lock-after 600
    screen-off-after 660
}

// Keyboard layout + key repeat. layout accepts comma lists ("us,es") and
// options can pair them with a toggle ("grp:alt_shift_toggle").
input {
    keyboard-layout "us"
    repeat-delay 200
    repeat-rate 25
    // Window-manager modifier. "super" (default), "alt", "ctrl", or "caps"
    // (Caps Lock becomes an extra Super — handy on 60% boards where Fn+Super
    // disables the real Super key).
    // mod-key "super"

    // Pointer / touchpad. Touchpads get tap-to-click + natural scroll by
    // default; uncomment to change. accel-speed is -1.0 (slow) … 1.0 (fast).
    // focus-follows-mouse moves keyboard focus to the window under the pointer.
    // natural-scroll #true
    // tap-to-click #true
    // accel-speed 0.0
    // disable-while-typing #true
    // focus-follows-mouse #false
}

// Window rules — match by app-id (exact, case-insensitive) and/or title
// (substring). Actions: workspace=N, float=#true/#false, opacity=0..1,
// fullscreen=#true.
// rules {
//     rule app-id="firefox" workspace=2
//     rule app-id="pavucontrol" float=#true
//     rule title="Picture-in-Picture" float=#true opacity=0.95
// }
"#;

// ── KDL schema ────────────────────────────────────────────────────────────────

#[derive(knus::Decode, Debug)]
pub struct Document {
    #[knus(child)]
    pub binds: Option<BindsBlock>,
    #[knus(child)]
    pub theme: Option<ThemeBlock>,
    #[knus(child)]
    pub idle: Option<IdleBlock>,
    #[knus(child)]
    pub input: Option<InputBlock>,
    /// Per-monitor arrangement. Each `output "NAME" { … }` node sets scale /
    /// position / mode for the matching connector (e.g. "eDP-1", "DP-2").
    #[knus(children(name = "output"))]
    pub outputs: Vec<OutputEntry>,
    /// Window rules: `rules { rule app-id="…" … }`.
    #[knus(child)]
    pub rules: Option<RulesBlock>,
}

#[derive(knus::Decode, Debug)]
pub struct RulesBlock {
    #[knus(children(name = "rule"))]
    pub entries: Vec<RuleEntry>,
}

/// One window rule. Matchers (`app-id`, `title`) select windows; the remaining
/// properties are what to do with a match. Example:
///   rule app-id="firefox" workspace=2
///   rule app-id="pavucontrol" float=#true
///   rule title="Picture-in-Picture" float=#true opacity=0.95
#[derive(knus::Decode, Debug)]
pub struct RuleEntry {
    /// Exact app_id / X11 class (case-insensitive).
    #[knus(property(name = "app-id"))]
    pub app_id: Option<String>,
    /// Substring of the window title (case-insensitive).
    #[knus(property)]
    pub title: Option<String>,
    /// Open the window on this workspace.
    #[knus(property)]
    pub workspace: Option<i64>,
    /// Force floating (true) or tiled (false).
    #[knus(property)]
    pub float: Option<bool>,
    /// Per-window opacity 0.0–1.0.
    #[knus(property)]
    pub opacity: Option<f64>,
    /// Open fullscreen.
    #[knus(property)]
    pub fullscreen: Option<bool>,
}

#[derive(knus::Decode, Debug)]
pub struct OutputEntry {
    /// Connector name as reported by `vendi-ctl output list` (e.g. "eDP-1").
    #[knus(argument)]
    pub name: String,
    /// Fractional scale (1.0, 1.5, 2.0 …). HiDPI displays usually want 1.5–2.
    #[knus(child, unwrap(argument))]
    pub scale: Option<f64>,
    /// Top-left position of this monitor in the global layout, logical px.
    #[knus(child, unwrap(argument))]
    pub x: Option<i64>,
    #[knus(child, unwrap(argument))]
    pub y: Option<i64>,
    /// Resolution + optional refresh, "2560x1440" or "2560x1440@165".
    #[knus(child, unwrap(argument))]
    pub mode: Option<String>,
}

#[derive(knus::Decode, Debug)]
pub struct IdleBlock {
    /// Seconds of inactivity before the session auto-locks (0 = never).
    /// knus kebab-cases the field, so this is the `lock-after` node.
    #[knus(child, unwrap(argument))]
    pub lock_after: Option<i64>,
    /// Seconds of inactivity before the displays power off via DPMS
    /// (0 = never). Any input wakes them.
    #[knus(child, unwrap(argument))]
    pub screen_off_after: Option<i64>,
    /// Seconds of inactivity before the video screensaver starts (0 = never).
    /// Any input dismisses it. The launcher self-skips on battery / no video.
    #[knus(child, unwrap(argument))]
    pub screensaver_after: Option<i64>,
}

#[derive(knus::Decode, Debug)]
pub struct InputBlock {
    /// xkb layout(s), e.g. "us" or "us,es" (comma-separated to toggle).
    #[knus(child, unwrap(argument))]
    pub keyboard_layout: Option<String>,
    /// xkb variant, e.g. "dvorak" or "intl".
    #[knus(child, unwrap(argument))]
    pub keyboard_variant: Option<String>,
    /// xkb options, e.g. "ctrl:nocaps,grp:alt_shift_toggle".
    #[knus(child, unwrap(argument))]
    pub keyboard_options: Option<String>,
    /// Which physical key acts as the window-manager modifier (the "super" in
    /// every bind). "super" (default) uses the Super/Win key. "alt"/"ctrl"
    /// remap every `super+…` bind onto that modifier instead. "caps" turns Caps
    /// Lock into an additional Super and leaves the binds untouched — ideal for
    /// 60% keyboards whose Fn layer can disable the real Super key.
    #[knus(child, unwrap(argument))]
    pub mod_key: Option<String>,
    /// Key repeat: ms before repeat starts.
    #[knus(child, unwrap(argument))]
    pub repeat_delay: Option<i64>,
    /// Key repeat: repeats per second.
    #[knus(child, unwrap(argument))]
    pub repeat_rate: Option<i64>,
    /// Reverse ("natural") scrolling (default: on for touchpads, off for mice).
    #[knus(child, unwrap(argument))]
    pub natural_scroll: Option<bool>,
    /// Touchpad tap-to-click (default on for touchpads).
    #[knus(child, unwrap(argument))]
    pub tap_to_click: Option<bool>,
    /// Pointer acceleration, -1.0 (slow) … 1.0 (fast). Default 0.0.
    #[knus(child, unwrap(argument))]
    pub accel_speed: Option<f64>,
    /// Touchpad: ignore taps/clicks while typing (default on).
    #[knus(child, unwrap(argument))]
    pub disable_while_typing: Option<bool>,
    /// Move keyboard focus to whatever window the pointer is over.
    #[knus(child, unwrap(argument))]
    pub focus_follows_mouse: Option<bool>,
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
    /// Default window opacity, 0.0–1.0 (1.0 = opaque). Applies to every
    /// window; the `cycle-opacity` bind overrides it per-window at runtime.
    #[knus(child, unwrap(argument))]
    pub opacity:    Option<f64>,
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
    pub opacity:    f32,
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
            opacity:    1.0,
        }
    }
}

/// Resolved per-output arrangement. Any field left None keeps the auto value
/// (preferred mode, scale 1, packed left-to-right).
#[derive(Debug, Clone)]
pub struct OutputCfg {
    pub name:     String,
    pub scale:    Option<f64>,
    pub position: Option<(i32, i32)>,
    /// (width, height, optional refresh in Hz)
    pub mode:     Option<(i32, i32, Option<u32>)>,
}

pub struct Config {
    pub keybinds: HashMap<Chord, Action>,
    /// Human-readable bind list in config order, user overrides applied.
    /// Served over IPC (`list-binds`) for the Super+K keybinds menu.
    pub keybinds_pretty: Vec<(String, String)>,
    pub theme:    Theme,
    /// Auto-lock after N seconds idle (0 = disabled).
    pub idle_lock_secs: u64,
    /// Power displays off (DPMS) after N seconds idle (0 = disabled).
    pub idle_screen_off_secs: u64,
    /// Start the video screensaver after N seconds idle (0 = disabled).
    pub idle_screensaver_secs: u64,
    /// Keyboard / xkb settings, applied at startup and on reload.
    pub kb_layout:  String,
    pub kb_variant: String,
    pub kb_options: String,
    pub repeat_delay: i32,
    pub repeat_rate:  i32,
    /// Pointer/touchpad. `None` = leave libinput's per-device default (so mice
    /// keep traditional scroll, touchpads keep tap+natural unless overridden).
    pub natural_scroll: Option<bool>,
    pub tap_to_click: Option<bool>,
    pub accel_speed: Option<f64>,
    pub disable_while_typing: Option<bool>,
    /// Sloppy focus: focus the window under the pointer on motion.
    pub focus_follows_mouse: bool,
    /// Per-monitor arrangement, keyed by connector name.
    pub outputs: Vec<OutputCfg>,
    /// User window rules, applied once when a window is first classified.
    pub window_rules: Vec<WindowRule>,
}

/// A compiled window rule. Empty matchers never match.
#[derive(Debug, Clone)]
pub struct WindowRule {
    pub app_id: Option<String>,   // exact, case-insensitive
    pub title:  Option<String>,   // substring, case-insensitive
    pub workspace:  Option<u32>,
    pub float:      Option<bool>,
    pub opacity:    Option<f32>,
    pub fullscreen: Option<bool>,
}

/// The merged effect of every rule that matched a window.
#[derive(Debug, Clone, Copy, Default)]
pub struct RuleEffect {
    pub workspace:  Option<u32>,
    pub float:      Option<bool>,
    pub opacity:    Option<f32>,
    pub fullscreen: Option<bool>,
}

impl RuleEffect {
    pub fn is_empty(&self) -> bool {
        self.workspace.is_none() && self.float.is_none()
            && self.opacity.is_none() && self.fullscreen.is_none()
    }
}

impl Config {
    /// Look up the arrangement for a connector by name, if the user set one.
    pub fn output_cfg(&self, name: &str) -> Option<&OutputCfg> {
        self.outputs.iter().find(|o| o.name == name)
    }

    /// Merge every window rule matching `app_id` (exact, case-insensitive) and
    /// `title` (substring, case-insensitive). Later rules win per-field.
    pub fn match_window(&self, app_id: &str, title: &str) -> RuleEffect {
        let mut eff = RuleEffect::default();
        for r in &self.window_rules {
            // A rule with no matcher matches nothing (avoids a global override).
            if r.app_id.is_none() && r.title.is_none() { continue; }
            let app_ok = r.app_id.as_deref()
                .is_none_or(|p| p.eq_ignore_ascii_case(app_id));
            let title_ok = r.title.as_deref()
                .is_none_or(|p| title.to_lowercase().contains(&p.to_lowercase()));
            if app_ok && title_ok {
                if r.workspace.is_some()  { eff.workspace  = r.workspace; }
                if r.float.is_some()      { eff.float      = r.float; }
                if r.opacity.is_some()    { eff.opacity    = r.opacity; }
                if r.fullscreen.is_some() { eff.fullscreen = r.fullscreen; }
            }
        }
        eff
    }

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

        // Window rules: built-in (none today) then user, in file order so user
        // rules apply last. Extracted up front while both docs are intact.
        let mut window_rules: Vec<WindowRule> = Vec::new();
        for blk in [default_doc.rules, user_doc.as_mut().and_then(|d| d.rules.take())]
            .into_iter().flatten()
        {
            for e in blk.entries {
                window_rules.push(WindowRule {
                    app_id: e.app_id,
                    title:  e.title,
                    workspace:  e.workspace.map(|w| w.max(1) as u32),
                    float:      e.float,
                    opacity:    e.opacity.map(|o| o.clamp(0.0, 1.0) as f32),
                    fullscreen: e.fullscreen,
                });
            }
        }

        // (chord, pretty chord, pretty action, action) in config order. A user
        // bind on an existing chord replaces that slot so the pretty list
        // shows the override, not both.
        let mut entries: Vec<(Chord, String, String, Action)> = Vec::new();
        let default_binds = default_doc.binds.map(|b| b.entries).unwrap_or_default();
        let user_binds = user_doc.as_mut()
            .and_then(|d| d.binds.take())
            .map(|b| b.entries)
            .unwrap_or_default();
        // Pull the idle/input blocks out now, before user_doc is consumed.
        let default_idle = default_doc.idle;
        let user_idle = user_doc.as_mut().and_then(|d| d.idle.take());
        let default_input = default_doc.input;
        let user_input = user_doc.as_mut().and_then(|d| d.input.take());
        // Which physical key the binds treat as "super". Read it now (without
        // consuming the input blocks) so it can rewrite the chords below.
        let mod_key = user_input.as_ref().and_then(|i| i.mod_key.clone())
            .or_else(|| default_input.as_ref().and_then(|i| i.mod_key.clone()))
            .unwrap_or_else(|| "super".into())
            .trim().to_ascii_lowercase();
        if !matches!(mod_key.as_str(), "super" | "alt" | "ctrl" | "caps") {
            tracing::warn!("unknown mod-key {mod_key:?}; using \"super\"");
        }
        let user_outputs = user_doc.as_mut()
            .map(|d| std::mem::take(&mut d.outputs))
            .unwrap_or_default();
        for entry in default_binds.into_iter().chain(user_binds) {
            let chord = remap_mod(
                parse_chord(&entry.chord)
                    .with_context(|| format!("parse chord {:?}", entry.chord))?,
                &mod_key,
            );
            let action = parse_action(&entry.action)
                .with_context(|| format!("parse action {:?}", entry.action))?;
            // Reflect the active mod in the human-readable chord (Super+K menu).
            let pretty_chord = remap_pretty(&entry.chord, &mod_key);
            match entries.iter_mut().find(|(c, ..)| *c == chord) {
                Some(slot) => *slot = (chord, pretty_chord, entry.action, action),
                None       => entries.push((chord, pretty_chord, entry.action, action)),
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
            if let Some(v) = t.opacity { theme.opacity = (v as f32).clamp(0.1, 1.0); }
        }

        // Runtime wallpaper switches (vendi-ctl wallpaper / the bar's picker)
        // persist to ~/.config/vendi/wallpaper — the strongest override.
        if let Some(p) = read_wallpaper_override() {
            theme.wallpaper = Some(p);
        }

        // Idle auto-lock + screen-off + screensaver: built-in default, then
        // user override. Screensaver fires before screen-off by default.
        let mut idle_lock_secs: u64 = 600;
        let mut idle_screen_off_secs: u64 = 660;
        let mut idle_screensaver_secs: u64 = 300;
        for blk in [default_idle, user_idle].into_iter().flatten() {
            if let Some(v) = blk.lock_after        { idle_lock_secs = v.max(0) as u64; }
            if let Some(v) = blk.screen_off_after  { idle_screen_off_secs = v.max(0) as u64; }
            if let Some(v) = blk.screensaver_after { idle_screensaver_secs = v.max(0) as u64; }
        }

        // Keyboard: defaults, then built-in config, then user override.
        let mut kb_layout = "us".to_string();
        let mut kb_variant = String::new();
        let mut kb_options = String::new();
        let mut repeat_delay = 200_i32;
        let mut repeat_rate = 25_i32;
        let mut natural_scroll: Option<bool> = None;
        let mut tap_to_click: Option<bool> = None;
        let mut accel_speed: Option<f64> = None;
        let mut disable_while_typing: Option<bool> = None;
        let mut focus_follows_mouse = false;
        for blk in [default_input, user_input].into_iter().flatten() {
            if let Some(v) = blk.keyboard_layout  { kb_layout = v; }
            if let Some(v) = blk.keyboard_variant { kb_variant = v; }
            if let Some(v) = blk.keyboard_options { kb_options = v; }
            if let Some(v) = blk.repeat_delay     { repeat_delay = v as i32; }
            if let Some(v) = blk.repeat_rate      { repeat_rate = v as i32; }
            if let Some(v) = blk.natural_scroll       { natural_scroll = Some(v); }
            if let Some(v) = blk.tap_to_click         { tap_to_click = Some(v); }
            if let Some(v) = blk.accel_speed          { accel_speed = Some(v); }
            if let Some(v) = blk.disable_while_typing { disable_while_typing = Some(v); }
            if let Some(v) = blk.focus_follows_mouse  { focus_follows_mouse = v; }
        }
        // "caps" mode keeps the super binds intact and instead makes Caps Lock
        // emit Super via xkb, so it works even when the real Super key is gone.
        if mod_key == "caps" && !kb_options.split(',').any(|o| o.trim() == "caps:super") {
            if kb_options.is_empty() { kb_options = "caps:super".into(); }
            else { kb_options.push_str(",caps:super"); }
        }

        // Output arrangement: blocks in vendiwm.kdl, then the vendi-ctl-managed
        // outputs.kdl wins (so live `vendi-ctl output set` / the Pro GUI persist
        // and survive a reload), overriding per connector name.
        let mut output_entries = user_outputs;
        if let Some(extra) = read_output_overrides()? {
            for e in extra {
                match output_entries.iter_mut().find(|o| o.name == e.name) {
                    Some(slot) => *slot = e,
                    None       => output_entries.push(e),
                }
            }
        }
        let outputs: Vec<OutputCfg> = output_entries.into_iter().map(|e| OutputCfg {
            name:     e.name,
            scale:    e.scale.filter(|s| *s > 0.0),
            position: match (e.x, e.y) {
                (Some(x), Some(y)) => Some((x as i32, y as i32)),
                _ => None,
            },
            mode:     e.mode.as_deref().and_then(parse_mode),
        }).collect();

        Ok(Self {
            keybinds, keybinds_pretty, theme, idle_lock_secs, idle_screen_off_secs,
            idle_screensaver_secs,
            kb_layout, kb_variant, kb_options, repeat_delay, repeat_rate,
            natural_scroll, tap_to_click, accel_speed, disable_while_typing,
            focus_follows_mouse, outputs, window_rules,
        })
    }
}

/// "WxH" or "WxH@Hz" → (width, height, optional refresh).
fn parse_mode(s: &str) -> Option<(i32, i32, Option<u32>)> {
    let (res, hz) = match s.split_once('@') {
        Some((r, h)) => (r, h.trim().parse::<u32>().ok()),
        None         => (s, None),
    };
    let (w, h) = res.split_once('x')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?, hz))
}

/// The vendi-ctl-managed arrangement overrides (~/.config/vendi/outputs.kdl).
/// Contains only `output "NAME" { … }` nodes; rewritten by `vendi-ctl output`.
fn read_output_overrides() -> Result<Option<Vec<OutputEntry>>> {
    let mut path = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(d) => PathBuf::from(d),
        None    => {
            let home = std::env::var_os("HOME")
                .ok_or_else(|| anyhow::anyhow!("HOME not set"))?;
            PathBuf::from(home).join(".config")
        }
    };
    path.push("vendi");
    path.push("outputs.kdl");
    match std::fs::read_to_string(&path) {
        Ok(text) => match knus::parse::<Document>("outputs.kdl", &text) {
            Ok(doc) => Ok(Some(doc.outputs)),
            // A broken overrides file must never brick the whole config (which
            // would take down keybinds, theme, everything). Warn and ignore.
            Err(e) => {
                tracing::warn!("ignoring malformed outputs.kdl: {e}");
                Ok(None)
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::Error::new(e).context("read outputs.kdl")),
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

/// Apply the configured `mod-key` to a parsed chord. "alt"/"ctrl" move the
/// Super requirement onto that modifier; "super"/"caps" leave it on Logo
/// ("caps" works by making Caps Lock emit Super via xkb instead).
fn remap_mod(mut c: Chord, mod_key: &str) -> Chord {
    if c.logo {
        match mod_key {
            "alt"  => { c.logo = false; c.alt  = true; }
            "ctrl" => { c.logo = false; c.ctrl = true; }
            _ => {}
        }
    }
    c
}

/// Mirror `remap_mod` for the human-readable chord shown in the keybinds menu,
/// e.g. "super+return" → "alt+return" when mod-key is "alt".
fn remap_pretty(chord: &str, mod_key: &str) -> String {
    match mod_key {
        "alt" | "ctrl" => chord.replacen("super", mod_key, 1),
        _ => chord.to_string(),
    }
}

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
        "kill"              => Action::Kill,
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
        "move-to-workspace-follow" => Action::MoveToWorkspaceFollow(rest.parse().context("workspace number")?),
        "workspace-next"    => Action::WorkspaceNext,
        "workspace-prev"    => Action::WorkspacePrev,
        "workspace-last"    => Action::WorkspaceLast,
        "reload-config"     => Action::ReloadConfig,
        "center-floating"   => Action::CenterFloating,
        "cycle-layout"      => Action::CycleLayout,
        "toggle-floating"   => Action::ToggleFloating,
        "fullscreen"        => Action::ToggleFullscreen,
        "overview"          => Action::ToggleOverview,
        "toggle-blur"       => Action::ToggleBlur,
        "cycle-opacity"     => Action::CycleOpacity,
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
