// Actions mode — `vendi-menu actions`.
//
// A centered vertical list of system verbs, no search bar: lock, screenshots,
// relaunch, suspend, reboot, shutdown. Arrow keys + Enter, digits for direct
// hits, Escape closes, mouse works. Same card language as the launcher.

use gtk4 as gtk;
use gtk::{gdk, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

pub struct ActionItem {
    pub glyph: &'static str,
    pub label: &'static str,
    pub cmd:   &'static str,
}

/// Default verbs. Everything is best-effort: missing tools just no-op.
pub const ACTIONS: &[ActionItem] = &[
    ActionItem { glyph: "\u{f033e}", label: "Lock",              cmd: "swaylock -f" },
    ActionItem { glyph: "\u{f0c4e}", label: "Screenshot region", cmd: "sh -c 'grim -g \"$(slurp)\" - | wl-copy'" },
    ActionItem { glyph: "\u{f0e51}", label: "Screenshot screen", cmd: "sh -c 'mkdir -p ~/Pictures && grim ~/Pictures/screenshot-$(date +%s).png'" },
    ActionItem { glyph: "\u{f0450}", label: "Relaunch session",  cmd: "pkill -x vendiwm" },
    ActionItem { glyph: "\u{f04b2}", label: "Suspend",           cmd: "systemctl suspend" },
    ActionItem { glyph: "\u{f0709}", label: "Restart",           cmd: "systemctl reboot" },
    ActionItem { glyph: "\u{f0425}", label: "Shutdown",          cmd: "systemctl poweroff" },
];

pub fn build_ui(app: &gtk::Application) {
    // Re-running while open toggles the menu closed.
    if let Some(window) = app.active_window() {
        window.close();
        return;
    }

    let window = gtk::ApplicationWindow::new(app);
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_namespace(Some("vendi-menu"));
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    window.set_default_width(340);
    window.add_css_class("menu");

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("menu-root");
    root.add_css_class("actions-root");

    let list = gtk::ListBox::new();
    list.add_css_class("results");
    list.add_css_class("actions");
    list.set_selection_mode(gtk::SelectionMode::Browse);

    for (i, action) in ACTIONS.iter().enumerate() {
        let row = gtk::ListBoxRow::new();
        row.add_css_class("hit");
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        let glyph = gtk::Label::new(Some(action.glyph));
        glyph.add_css_class("action-glyph");
        let label = gtk::Label::new(Some(action.label));
        label.add_css_class("hit-name");
        label.set_xalign(0.0);
        label.set_hexpand(true);
        let index = gtk::Label::new(Some(&(i + 1).to_string()));
        index.add_css_class("action-index");
        line.append(&glyph);
        line.append(&label);
        line.append(&index);
        row.set_child(Some(&line));
        list.append(&row);
    }
    if let Some(first) = list.row_at_index(0) {
        list.select_row(Some(&first));
    }

    root.append(&list);
    window.set_child(Some(&root));

    let run = |idx: i32| {
        if let Some(action) = ACTIONS.get(idx as usize) {
            tracing::info!(cmd = %action.cmd, "action");
            let _ = std::process::Command::new("sh").arg("-c").arg(action.cmd).spawn();
        }
    };

    {
        let app = app.clone();
        list.connect_row_activated(move |_, row| {
            run(row.index());
            app.quit();
        });
    }

    let keys = gtk::EventControllerKey::new();
    {
        let (app, list) = (app.clone(), list.clone());
        keys.connect_key_pressed(move |_, key, _, _| {
            match key {
                gdk::Key::Escape => { app.quit(); glib::Propagation::Stop }
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    if let Some(row) = list.selected_row() {
                        run(row.index());
                    }
                    app.quit();
                    glib::Propagation::Stop
                }
                gdk::Key::Down | gdk::Key::Up | gdk::Key::Tab => {
                    let delta = if key == gdk::Key::Up { -1 } else { 1 };
                    let cur = list.selected_row().map(|r| r.index()).unwrap_or(0);
                    let next = (cur + delta).rem_euclid(ACTIONS.len() as i32);
                    if let Some(row) = list.row_at_index(next) {
                        list.select_row(Some(&row));
                    }
                    glib::Propagation::Stop
                }
                k => {
                    // Digit shortcuts: 1..9 fire the row directly.
                    if let Some(d) = k.to_unicode().and_then(|c| c.to_digit(10)) {
                        if d >= 1 && (d as usize) <= ACTIONS.len() {
                            run(d as i32 - 1);
                            app.quit();
                            return glib::Propagation::Stop;
                        }
                    }
                    glib::Propagation::Proceed
                }
            }
        });
    }
    window.add_controller(keys);
    window.present();
}
