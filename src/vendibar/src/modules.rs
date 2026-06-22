// System modules: clock, music, volume, network, battery.
//
// Each returns a finished widget that keeps itself updated with glib timers.
// Everything reads cheap sources (/sys, wpctl, playerctl) — no daemons, no
// D-Bus bindings yet.

use gtk4 as gtk;
use gtk::{glib, prelude::*};

/// Center clock — "Tue Jun 10  21:48".
pub fn clock() -> gtk::Label {
    let label = gtk::Label::new(None);
    label.add_css_class("clock");
    let tick = {
        let label = label.clone();
        move || label.set_text(&chrono::Local::now().format("%a %b %-d  %H:%M").to_string())
    };
    tick();
    glib::timeout_add_seconds_local(5, move || { tick(); glib::ControlFlow::Continue });
    label
}

/// MPRIS now-playing widget: a tiny animated equalizer + "artist – title".
/// Hidden entirely when no player is running. Click toggles play/pause,
/// scroll skips next/previous. Polls playerctl — same zero-daemon approach
/// as the other modules.
pub fn music() -> gtk::Box {
    use std::cell::Cell;
    use std::rc::Rc;

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    row.add_css_class("music");
    row.set_visible(false);

    // ── the equalizer ────────────────────────────────────────────────────────
    // Four cairo bars bobbing on offset sine waves while playing; calm low
    // bars while paused. The clock only ticks while music actually plays.
    let eq = gtk::DrawingArea::new();
    eq.set_content_width(15);
    eq.set_content_height(13);
    eq.set_valign(gtk::Align::Center);
    let phase   = Rc::new(Cell::new(0.0_f64));
    let playing = Rc::new(Cell::new(false));
    {
        let (phase, playing) = (phase.clone(), playing.clone());
        let (ar, ag, ab) = accent_rgb();
        eq.set_draw_func(move |_, cr, w, h| {
            const BARS: usize = 4;
            let (w, h) = (w as f64, h as f64);
            cr.set_source_rgba(ar, ag, ab, 1.0);
            let slot = w / BARS as f64;
            let bw = slot * 0.62;
            let t = phase.get();
            for i in 0..BARS {
                let level = if playing.get() {
                    0.25 + 0.75 * (t * 1.0 + i as f64 * 1.9).sin().abs()
                } else {
                    0.22
                };
                let bh = (level * h).max(2.0);
                let x = i as f64 * slot;
                // Rounded caps: a rectangle with a small radius via arcs is
                // overkill at this size — a 1px-radius rectangle reads fine.
                cr.rectangle(x, h - bh, bw, bh);
            }
            let _ = cr.fill();
        });
    }
    {
        let (phase, playing, eq) = (phase.clone(), playing.clone(), eq.clone());
        glib::timeout_add_local(std::time::Duration::from_millis(90), move || {
            if playing.get() && eq.is_visible() {
                phase.set(phase.get() + 0.55);
                eq.queue_draw();
            }
            glib::ControlFlow::Continue
        });
    }

    let label = gtk::Label::new(None);
    label.add_css_class("music-label");
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_max_width_chars(28);

    row.append(&eq);
    row.append(&label);

    // ── state poll ───────────────────────────────────────────────────────────
    let refresh: Rc<dyn Fn()> = {
        let (playing, row, label, eq) = (playing.clone(), row.clone(), label.clone(), eq.clone());
        Rc::new(move || {
            let status = read_cmd(&["playerctl", "status"])
                .map(|s| s.trim().to_string());
            match status.as_deref() {
                Some("Playing") | Some("Paused") => {
                    let is_playing = status.as_deref() == Some("Playing");
                    if playing.replace(is_playing) != is_playing {
                        eq.queue_draw();
                    }
                    let artist = read_cmd(&["playerctl", "metadata", "artist"])
                        .map(|s| s.trim().to_string()).unwrap_or_default();
                    let title = read_cmd(&["playerctl", "metadata", "title"])
                        .map(|s| s.trim().to_string()).unwrap_or_default();
                    let text = match (artist.is_empty(), title.is_empty()) {
                        (false, false) => format!("{artist} – {title}"),
                        (true, false)  => title,
                        _              => "playing".into(),
                    };
                    label.set_text(&text);
                    row.set_visible(true);
                }
                _ => {
                    playing.set(false);
                    row.set_visible(false);
                }
            }
        })
    };
    refresh();
    {
        let refresh = refresh.clone();
        glib::timeout_add_seconds_local(2, move || {
            refresh();
            glib::ControlFlow::Continue
        });
    }

    // ── controls ─────────────────────────────────────────────────────────────
    let click = gtk::GestureClick::new();
    {
        let refresh = refresh.clone();
        click.connect_released(move |_, _, _, _| {
            run(&["playerctl", "play-pause"]);
            refresh();
        });
    }
    row.add_controller(click);

    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    {
        let refresh = refresh.clone();
        scroll.connect_scroll(move |_, _, dy| {
            run(&["playerctl", if dy < 0.0 { "next" } else { "previous" }]);
            refresh();
            glib::Propagation::Stop
        });
    }
    row.add_controller(scroll);

    row
}

/// Volume readout via wpctl. Click toggles mute, scroll adjusts ±2%.
pub fn volume() -> gtk::Label {
    let label = gtk::Label::new(None);
    label.add_css_class("volume");
    refresh_volume(&label);

    {
        let label = label.clone();
        glib::timeout_add_seconds_local(3, move || {
            refresh_volume(&label);
            glib::ControlFlow::Continue
        });
    }

    let click = gtk::GestureClick::new();
    {
        let label = label.clone();
        click.connect_released(move |_, _, _, _| {
            run(&["wpctl", "set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]);
            refresh_volume(&label);
        });
    }
    label.add_controller(click);

    // Right-click: the audio output picker, floating.
    let rclick = gtk::GestureClick::new();
    rclick.set_button(3);
    rclick.connect_released(|_, _, _, _| spawn_float(&["vendi", "audio"]));
    label.add_controller(rclick);

    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    {
        let label = label.clone();
        scroll.connect_scroll(move |_, _, dy| {
            let dir = if dy < 0.0 { "2%+" } else { "2%-" };
            run(&["wpctl", "set-volume", "-l", "1.0", "@DEFAULT_AUDIO_SINK@", dir]);
            refresh_volume(&label);
            // Accent flash so the change registers at a glance.
            label.add_css_class("bump");
            let label = label.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
                label.remove_css_class("bump");
            });
            glib::Propagation::Stop
        });
    }
    label.add_controller(scroll);

    label
}

fn refresh_volume(label: &gtk::Label) {
    // "Volume: 0.45" or "Volume: 0.45 [MUTED]"
    let out = read_cmd(&["wpctl", "get-volume", "@DEFAULT_AUDIO_SINK@"]);
    let Some(out) = out else {
        label.set_text("\u{f057f} --");   // 󰕿
        return;
    };
    let muted = out.contains("[MUTED]");
    let pct = out.split_whitespace().nth(1)
        .and_then(|v| v.parse::<f32>().ok())
        .map(|v| (v * 100.0).round() as u32)
        .unwrap_or(0);
    let icon = if muted { "\u{f0581}" }                  // 󰖁
        else if pct >= 60 { "\u{f057e}" }                // 󰕾
        else if pct >= 25 { "\u{f0580}" }                // 󰖀
        else { "\u{f057f}" };                            // 󰕿
    label.set_text(&format!("{icon} {pct}%"));
    if muted { label.add_css_class("muted"); } else { label.remove_css_class("muted"); }
}

/// Network state from /sys/class/net — icon only, macOS style.
/// Hover names the connected network; click summons the wifi TUI.
pub fn network() -> gtk::Label {
    let label = gtk::Label::new(None);
    label.add_css_class("network");
    refresh_network(&label);
    {
        let label = label.clone();
        glib::timeout_add_seconds_local(5, move || {
            refresh_network(&label);
            glib::ControlFlow::Continue
        });
    }
    let click = gtk::GestureClick::new();
    click.connect_released(|_, _, _, _| spawn_float(&["vendi", "wifi"]));
    label.add_controller(click);
    label
}

fn refresh_network(label: &gtk::Label) {
    let mut icon = "\u{f05aa}";   // 󰖪 offline
    let mut up = false;
    let mut wifi = false;
    if let Ok(entries) = std::fs::read_dir("/sys/class/net") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "lo" { continue; }
            let operstate = std::fs::read_to_string(entry.path().join("operstate"))
                .unwrap_or_default();
            if operstate.trim() == "up" {
                up = true;
                wifi = name.starts_with('w');
                icon = if wifi { "\u{f05a9}" }   // 󰖩 wifi
                       else { "\u{f0200}" };      // 󰈀 ethernet
                if wifi { break; }   // prefer showing wifi
            }
        }
    }
    let tooltip = if !up {
        "no network — click to connect".to_string()
    } else if wifi {
        // "yes:MySSID" line from the active wifi list.
        read_cmd(&["nmcli", "-t", "-f", "active,ssid", "dev", "wifi"])
            .and_then(|out| out.lines()
                .find_map(|l| l.strip_prefix("yes:").map(str::to_string)))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "wifi".into())
    } else {
        "wired connection".to_string()
    };
    label.set_tooltip_text(Some(&tooltip));
    label.set_text(icon);
    if up { label.remove_css_class("offline"); } else { label.add_css_class("offline"); }
}

/// Bluetooth — hidden when the machine has no adapter. Hover names the
/// connected devices; click summons the bluetooth TUI.
pub fn bluetooth() -> Option<gtk::Label> {
    std::fs::read_dir("/sys/class/bluetooth").ok()?.flatten().next()?;
    let label = gtk::Label::new(None);
    label.add_css_class("bluetooth");
    refresh_bluetooth(&label);
    {
        let label = label.clone();
        glib::timeout_add_seconds_local(10, move || {
            refresh_bluetooth(&label);
            glib::ControlFlow::Continue
        });
    }
    let click = gtk::GestureClick::new();
    click.connect_released(|_, _, _, _| spawn_float(&["vendi", "bt"]));
    label.add_controller(click);
    Some(label)
}

fn refresh_bluetooth(label: &gtk::Label) {
    let powered = read_cmd(&["bluetoothctl", "show"])
        .map(|s| s.contains("Powered: yes"))
        .unwrap_or(false);
    // "Device AA:BB:.. Name" per connected device.
    let connected: Vec<String> = read_cmd(&["bluetoothctl", "devices", "Connected"])
        .map(|out| out.lines()
            .filter_map(|l| l.splitn(3, ' ').nth(2).map(str::to_string))
            .collect())
        .unwrap_or_default();
    let (icon, tooltip) = if !powered {
        ("\u{f00b2}", "bluetooth off — click to manage".to_string())   // 󰂲
    } else if connected.is_empty() {
        ("\u{f00af}", "bluetooth on — click to connect".to_string())   // 󰂯
    } else {
        ("\u{f00b1}", connected.join(", "))                            // 󰂱
    };
    label.set_text(icon);
    label.set_tooltip_text(Some(&tooltip));
    if powered { label.remove_css_class("muted"); } else { label.add_css_class("muted"); }
}

/// Run a vendi TUI in a floating terminal (vendiwm floats app_id vendi-float).
fn spawn_float(cmd: &[&str]) {
    let _ = std::process::Command::new("kitty")
        .args(["--class", "vendi-float", "-e"])
        .args(cmd)
        .spawn();
}

/// Battery from /sys/class/power_supply. Returns None when there is no
/// battery (desktops, VMs) so the module disappears entirely. Icon only —
/// percentage lives in the hover tooltip; click opens the power profile menu.
pub fn battery() -> Option<gtk::Label> {
    battery_path()?;
    let label = gtk::Label::new(None);
    label.add_css_class("battery");
    refresh_battery(&label);
    {
        let label = label.clone();
        glib::timeout_add_seconds_local(30, move || {
            refresh_battery(&label);
            glib::ControlFlow::Continue
        });
    }
    let click = gtk::GestureClick::new();
    click.connect_released(|_, _, _, _| {
        let _ = std::process::Command::new("vendi-menu").arg("power").spawn();
    });
    label.add_controller(click);
    Some(label)
}

fn battery_path() -> Option<std::path::PathBuf> {
    for entry in std::fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
        let path = entry.path();
        let kind = std::fs::read_to_string(path.join("type")).unwrap_or_default();
        if kind.trim() == "Battery" { return Some(path); }
    }
    None
}

fn refresh_battery(label: &gtk::Label) {
    let Some(path) = battery_path() else { return };
    let pct = std::fs::read_to_string(path.join("capacity")).ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);
    let charging = std::fs::read_to_string(path.join("status"))
        .map(|s| s.trim() == "Charging")
        .unwrap_or(false);
    let icon = if charging { "\u{f0084}" }     // 󰂄
        else {
            // 󰁺 (10%) … 󰁹 (100%), one glyph per decile.
            const LEVELS: [&str; 10] = [
                "\u{f007a}", "\u{f007b}", "\u{f007c}", "\u{f007d}", "\u{f007e}",
                "\u{f007f}", "\u{f0080}", "\u{f0081}", "\u{f0082}", "\u{f0079}",
            ];
            LEVELS[((pct.min(100).saturating_sub(1)) / 10) as usize]
        };
    label.set_text(icon);
    label.set_tooltip_text(Some(&format!(
        "{pct}%{} — click for power profile",
        if charging { " · charging" } else { "" }
    )));
    if pct <= 15 && !charging { label.add_css_class("low"); }
    else { label.remove_css_class("low"); }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Theme accent for cairo-drawn widgets (logo, equalizer). `vendi theme`
/// writes ACCENT_HEX into ~/.config/vendi/theme-state; default is Mauve.
pub fn accent_rgb() -> (f64, f64, f64) {
    let path = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .map(|p| p.join("vendi/theme-state"));
    if let Some(text) = path.and_then(|p| std::fs::read_to_string(p).ok()) {
        for line in text.lines() {
            if let Some(hex) = line.trim().strip_prefix("ACCENT_HEX=") {
                let hex = hex.trim();
                if hex.len() == 6 {
                    let p = |i| u8::from_str_radix(&hex[i..i + 2], 16).ok();
                    if let (Some(r), Some(g), Some(b)) = (p(0), p(2), p(4)) {
                        return (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
                    }
                }
            }
        }
    }
    (0.796, 0.651, 0.969)   // mauve
}

fn read_cmd(argv: &[&str]) -> Option<String> {
    let out = std::process::Command::new(argv[0]).args(&argv[1..]).output().ok()?;
    if !out.status.success() { return None; }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn run(argv: &[&str]) {
    let _ = std::process::Command::new(argv[0]).args(&argv[1..]).status();
}
