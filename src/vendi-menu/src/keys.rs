// Keys mode — `vendi-menu keys` (Super+K).
//
// A searchable cheatsheet of every active keybind, fetched live from the
// compositor over IPC (defaults merged with the user's overrides). Same card
// language as the launcher: search pill on top, scrollable list below.

use gtk4 as gtk;
use gtk::{gdk, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::ipc;

const WIDTH: i32 = 560;
const LIST_HEIGHT: i32 = 420;

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
    window.set_anchor(Edge::Top, true);
    window.set_margin(Edge::Top, 160);
    window.set_default_width(WIDTH);
    window.add_css_class("menu");

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("menu-root");

    let entry = gtk::SearchEntry::new();
    entry.add_css_class("search");
    entry.set_placeholder_text(Some("Keybinds"));
    entry.set_hexpand(true);

    let list = gtk::ListBox::new();
    list.add_css_class("results");
    list.set_selection_mode(gtk::SelectionMode::None);

    let binds = ipc::list_binds();
    if binds.is_empty() {
        let row = gtk::ListBoxRow::new();
        row.add_css_class("hit");
        let label = gtk::Label::new(Some("compositor IPC unavailable"));
        label.add_css_class("hit-comment");
        row.set_child(Some(&label));
        list.append(&row);
    }
    for (chord, action) in &binds {
        list.append(&bind_row(chord, action));
    }

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_max_content_height(LIST_HEIGHT);
    scroll.set_propagate_natural_height(true);
    scroll.set_child(Some(&list));

    root.append(&entry);
    root.append(&scroll);
    window.set_child(Some(&root));

    // Type → filter rows by chord or action substring.
    {
        let list = list.clone();
        entry.connect_search_changed(move |entry| {
            let query = entry.text().to_lowercase();
            let mut i = 0;
            while let Some(row) = list.row_at_index(i) {
                let hay: Option<std::ptr::NonNull<String>> = unsafe { row.data("hay") };
                let visible = match hay {
                    Some(h) => unsafe { h.as_ref() }.contains(&query),
                    None => true,
                };
                row.set_visible(query.is_empty() || visible);
                i += 1;
            }
        });
    }

    {
        let app = app.clone();
        entry.connect_stop_search(move |_| app.quit());
    }
    let keys = gtk::EventControllerKey::new();
    {
        let app = app.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                app.quit();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    window.add_controller(keys);

    window.present();
    entry.grab_focus();
}

fn bind_row(chord: &str, action: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("hit");
    row.set_activatable(false);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let chord_label = gtk::Label::new(Some(&pretty_chord(chord)));
    chord_label.add_css_class("key-chord");
    chord_label.set_xalign(0.0);
    chord_label.set_width_chars(24);
    let action_label = gtk::Label::new(Some(action));
    action_label.add_css_class("key-action");
    action_label.set_xalign(0.0);
    action_label.set_hexpand(true);
    action_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    line.append(&chord_label);
    line.append(&action_label);
    row.set_child(Some(&line));
    unsafe { row.set_data("hay", format!("{chord} {action}").to_lowercase()); }
    row
}

/// "super+shift+left" → "Super + Shift + ←"
fn pretty_chord(chord: &str) -> String {
    chord
        .split('+')
        .map(|part| match part.trim().to_ascii_lowercase().as_str() {
            "super"  => "Super".into(),
            "ctrl"   => "Ctrl".into(),
            "alt"    => "Alt".into(),
            "shift"  => "Shift".into(),
            "return" => "Enter".into(),
            "space"  => "Space".into(),
            "escape" => "Esc".into(),
            "left"   => "\u{2190}".into(),
            "right"  => "\u{2192}".into(),
            "up"     => "\u{2191}".into(),
            "down"   => "\u{2193}".into(),
            "tab"    => "Tab".into(),
            "print"  => "PrtSc".into(),
            other if other.len() == 1 => other.to_uppercase(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" + ")
}
