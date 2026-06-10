// Settings mode — `vendi-menu settings`.
//
// Real controls instead of editing vendiwm.kdl by hand: spin buttons for
// gap/border/radius, a switch for blur. Apply writes the theme block via
// the same patcher the wallpaper menu uses, then relaunches the session.
// The file stays the source of truth — power users can still edit it.

use gtk4 as gtk;
use gtk::{gdk, glib, prelude::*};
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};

use crate::actions::{config_path, relaunch, set_theme_key};

const WIDTH: i32 = 420;

#[derive(Clone, Copy)]
struct Current {
    gap:    f64,
    border: f64,
    radius: f64,
    blur:   bool,
}

/// Read the current theme-block values (compositor defaults when unset).
fn read_current() -> Current {
    let mut cur = Current { gap: 10.0, border: 2.0, radius: 12.0, blur: true };
    let Ok(text) = std::fs::read_to_string(config_path()) else { return cur };
    let mut in_theme = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("theme") && t.contains('{') { in_theme = true; continue; }
        if in_theme {
            if t == "}" { break; }
            let mut it = t.split_whitespace();
            match (it.next(), it.next()) {
                (Some("gap"), Some(v))    => cur.gap    = v.parse().unwrap_or(cur.gap),
                (Some("border"), Some(v)) => cur.border = v.parse().unwrap_or(cur.border),
                (Some("radius"), Some(v)) => cur.radius = v.parse().unwrap_or(cur.radius),
                (Some("blur"), Some(v))   => cur.blur   = v != "false",
                _ => {}
            }
        }
    }
    cur
}

fn row(label: &str, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    line.add_css_class("setting-row");
    let name = gtk::Label::new(Some(label));
    name.add_css_class("hit-name");
    name.set_xalign(0.0);
    name.set_hexpand(true);
    line.append(&name);
    line.append(control);
    line
}

fn spin(value: f64, max: f64, step: f64) -> gtk::SpinButton {
    let s = gtk::SpinButton::with_range(0.0, max, step);
    s.set_value(value);
    s.add_css_class("setting-spin");
    s
}

pub fn build_ui(app: &gtk::Application) {
    // Re-running while open toggles the panel closed.
    if let Some(window) = app.active_window() {
        window.close();
        return;
    }

    let window = gtk::ApplicationWindow::new(app);
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_namespace(Some("vendi-menu"));
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    window.set_default_width(WIDTH);
    window.add_css_class("menu");

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("menu-root");
    root.add_css_class("actions-root");

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("page-title");
    let glyph = gtk::Label::new(Some("\u{f0493}"));
    glyph.add_css_class("action-glyph");
    let title = gtk::Label::new(Some("Settings"));
    title.add_css_class("page-name");
    title.set_xalign(0.0);
    title.set_hexpand(true);
    header.append(&glyph);
    header.append(&title);
    root.append(&header);

    let cur = read_current();
    let gap    = spin(cur.gap, 40.0, 2.0);
    let border = spin(cur.border, 8.0, 1.0);
    let radius = spin(cur.radius, 24.0, 2.0);
    let blur   = gtk::Switch::new();
    blur.set_active(cur.blur);
    blur.set_valign(gtk::Align::Center);
    blur.add_css_class("setting-switch");

    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.add_css_class("results");
    list.append(&row("Window gap", &gap));
    list.append(&row("Border width", &border));
    list.append(&row("Corner radius", &radius));
    list.append(&row("Menu blur", &blur));

    // Apply writes only keys that changed, then relaunches the session so
    // the compositor re-reads its config.
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    buttons.add_css_class("setting-actions");
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let apply = gtk::Button::with_label("Apply + Reload");
    apply.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&apply);
    list.append(&buttons);

    root.append(&list);
    window.set_child(Some(&root));

    {
        let app = app.clone();
        cancel.connect_clicked(move |_| app.quit());
    }
    {
        let app = app.clone();
        let (gap, border, radius, blur) = (gap.clone(), border.clone(), radius.clone(), blur.clone());
        apply.connect_clicked(move |_| {
            let set_i = |key: &str, v: f64, old: f64| {
                if (v - old).abs() > 0.01 || true {
                    set_theme_key(key, Some(&format!("{}", v as i64)));
                }
            };
            set_i("gap", gap.value(), cur.gap);
            set_i("border", border.value(), cur.border);
            set_i("radius", radius.value(), cur.radius);
            set_theme_key("blur", Some(if blur.is_active() { "true" } else { "false" }));
            relaunch();
            app.quit();
        });
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
    let _ = glib::timeout_add_local_once(std::time::Duration::from_millis(200), {
        let win = window.clone();
        move || win.set_default_size(WIDTH, -1)
    });
}
