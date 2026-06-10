// System modules: clock, volume, network, battery.
//
// Each returns a finished widget that keeps itself updated with glib timers.
// Everything reads cheap sources (/sys, wpctl) — no daemons, no D-Bus yet.

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

    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    {
        let label = label.clone();
        scroll.connect_scroll(move |_, _, dy| {
            let dir = if dy < 0.0 { "2%+" } else { "2%-" };
            run(&["wpctl", "set-volume", "-l", "1.0", "@DEFAULT_AUDIO_SINK@", dir]);
            refresh_volume(&label);
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
    label
}

fn refresh_network(label: &gtk::Label) {
    let mut icon = "\u{f05aa}";   // 󰖪 offline
    let mut up = false;
    if let Ok(entries) = std::fs::read_dir("/sys/class/net") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "lo" { continue; }
            let operstate = std::fs::read_to_string(entry.path().join("operstate"))
                .unwrap_or_default();
            if operstate.trim() == "up" {
                up = true;
                icon = if name.starts_with('w') { "\u{f05a9}" }   // 󰖩 wifi
                       else { "\u{f0200}" };                       // 󰈀 ethernet
                if name.starts_with('w') { break; }   // prefer showing wifi
            }
        }
    }
    label.set_text(icon);
    if up { label.remove_css_class("offline"); } else { label.add_css_class("offline"); }
}

/// Battery from /sys/class/power_supply. Returns None when there is no
/// battery (desktops, VMs) so the module disappears entirely.
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
    label.set_text(&format!("{icon} {pct}%"));
    if pct <= 15 && !charging { label.add_css_class("low"); }
    else { label.remove_css_class("low"); }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn read_cmd(argv: &[&str]) -> Option<String> {
    let out = std::process::Command::new(argv[0]).args(&argv[1..]).output().ok()?;
    if !out.status.success() { return None; }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn run(argv: &[&str]) {
    let _ = std::process::Command::new(argv[0]).args(&argv[1..]).status();
}
