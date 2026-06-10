// vendi-menu — the vendiOS launcher.
//
// Spotlight-style: a single rounded search pill floating near the top of the
// screen; a results panel slides out underneath as you type. GTK4 +
// gtk4-layer-shell (Overlay layer, exclusive keyboard). All visuals in
// style.css, animations via GtkRevealer + CSS transitions.
//
// Sources searched today: .desktop applications. (Calculator, files, power
// verbs land here later — the design reserves the space.)

mod actions;
mod apps;
mod ipc;
mod keys;

use gtk4 as gtk;
use gtk::{gdk, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

const APP_ID:    &str = "os.vendi.menu";
const WIDTH:     i32  = 600;
const MAX_HITS:  usize = 8;

fn main() -> glib::ExitCode {
    // The menu is a small surface — cairo software rendering hits 60fps
    // everywhere and avoids GL probing (slow/broken in VMs). Respect an
    // explicit user override.
    if std::env::var_os("GSK_RENDERER").is_none() {
        // SAFETY: before gtk::init, single-threaded.
        unsafe { std::env::set_var("GSK_RENDERER", "cairo"); }
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "vendi_menu=info".into()),
        )
        .init();

    // `vendi-menu actions` = the nested system menu (no search bar);
    // `vendi-menu keys` = the keybind cheatsheet. Each mode gets its own
    // application id so the menus toggle independently.
    let mode = std::env::args().nth(1).unwrap_or_default();
    let (app_id, build): (&str, fn(&gtk::Application)) = match mode.as_str() {
        "actions" => ("os.vendi.menu.actions", actions::build_ui),
        "keys"    => ("os.vendi.menu.keys",    keys::build_ui),
        _         => (APP_ID,                  build_ui),
    };
    let app = gtk::Application::builder().application_id(app_id).build();
    app.connect_startup(|_| load_css());
    app.connect_activate(build);
    app.run_with_args::<&str>(&[])
}

/// Default theme ships in the binary; `vendi theme` generates an override at
/// ~/.config/vendi/vendi-menu.css which replaces it wholesale.
fn load_css() {
    let provider = gtk::CssProvider::new();
    let user_css = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .map(|p| p.join("vendi/vendi-menu.css"));
    match user_css.filter(|p| p.exists()) {
        Some(path) => provider.load_from_path(&path),
        None => provider.load_from_string(include_str!("style.css")),
    }
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("no display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn build_ui(app: &gtk::Application) {
    // Second activation while open = toggle off (Super+D toggles the menu).
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
    window.set_margin(Edge::Top, 220);
    window.set_default_width(WIDTH);
    window.add_css_class("menu");

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("menu-root");

    // ── the pill ────────────────────────────────────────────────────────────
    let entry = gtk::SearchEntry::new();
    entry.add_css_class("search");
    entry.set_placeholder_text(Some("Search"));
    entry.set_hexpand(true);

    // ── the results panel, hidden until there's a query ─────────────────────
    let revealer = gtk::Revealer::new();
    revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    revealer.set_transition_duration(180);
    let list = gtk::ListBox::new();
    list.add_css_class("results");
    list.set_selection_mode(gtk::SelectionMode::Browse);
    revealer.set_child(Some(&list));

    root.append(&entry);
    root.append(&revealer);
    window.set_child(Some(&root));

    let index = std::rc::Rc::new(apps::load());
    tracing::info!(apps = index.len(), "desktop index loaded");

    // Type → search → fill list → slide the panel open.
    {
        let (list, revealer, index) = (list.clone(), revealer.clone(), index.clone());
        let win = window.clone();
        entry.connect_search_changed(move |entry| {
            let query = entry.text().to_lowercase();
            while let Some(row) = list.first_child() { list.remove(&row); }
            if query.trim().is_empty() {
                revealer.set_reveal_child(false);
                win.add_css_class("collapsed");
                // Layer-shell windows don't shrink on their own: once the
                // collapse animation ends, hide the (now empty) revealer so
                // the window re-measures down to just the pill.
                let (revealer, win) = (revealer.clone(), win.clone());
                glib::timeout_add_local_once(std::time::Duration::from_millis(200), move || {
                    if !revealer.reveals_child() {
                        revealer.set_visible(false);
                        win.set_default_size(WIDTH, -1);
                    }
                });
                return;
            }
            let hits = apps::search(&index, &query, MAX_HITS);
            for app in &hits {
                list.append(&result_row(app));
            }
            if let Some(first) = list.row_at_index(0) {
                list.select_row(Some(&first));
            }
            revealer.set_visible(true);
            revealer.set_reveal_child(!hits.is_empty());
            win.remove_css_class("collapsed");
        });
    }

    // Escape: SearchEntry consumes the key itself and emits stop-search.
    {
        let app = app.clone();
        entry.connect_stop_search(move |_| app.quit());
    }

    // Enter → launch selection (or top hit).
    {
        let (list, app) = (list.clone(), app.clone());
        entry.connect_activate(move |_| {
            let row = list.selected_row().or_else(|| list.row_at_index(0));
            if let Some(row) = row {
                launch_row(&row);
                app.quit();
            }
        });
    }
    // Click a row → launch it.
    {
        let app = app.clone();
        list.connect_row_activated(move |_, row| {
            launch_row(row);
            app.quit();
        });
    }

    // Escape closes; Up/Down move the selection without leaving the entry.
    let keys = gtk::EventControllerKey::new();
    {
        let (app, list) = (app.clone(), list.clone());
        keys.connect_key_pressed(move |_, key, _, _| {
            match key {
                gdk::Key::Escape => { app.quit(); glib::Propagation::Stop }
                gdk::Key::Down | gdk::Key::Up => {
                    let delta = if key == gdk::Key::Down { 1 } else { -1 };
                    let cur = list.selected_row().map(|r| r.index()).unwrap_or(-1);
                    if let Some(row) = list.row_at_index((cur + delta).max(0)) {
                        list.select_row(Some(&row));
                    }
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
    }
    window.add_controller(keys);

    window.add_css_class("collapsed");
    window.present();
    entry.grab_focus();
}

/// One result row: icon + name (+ comment, dimmed, right-aligned).
fn result_row(app: &apps::DesktopApp) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("hit");
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let icon = gtk::Image::from_icon_name(app.icon.as_deref().unwrap_or("application-x-executable"));
    icon.set_pixel_size(24);
    let name = gtk::Label::new(Some(&app.name));
    name.add_css_class("hit-name");
    name.set_xalign(0.0);
    line.append(&icon);
    line.append(&name);
    if let Some(comment) = &app.comment {
        let c = gtk::Label::new(Some(comment));
        c.add_css_class("hit-comment");
        c.set_ellipsize(gtk::pango::EllipsizeMode::End);
        c.set_hexpand(true);
        c.set_halign(gtk::Align::End);
        line.append(&c);
    }
    row.set_child(Some(&line));
    // Stash the exec line on the row for launch_row.
    unsafe { row.set_data("exec", app.exec.clone()); }
    row
}

fn launch_row(row: &gtk::ListBoxRow) {
    let exec: Option<std::ptr::NonNull<String>> = unsafe { row.data("exec") };
    let Some(exec) = exec else { return };
    let exec = unsafe { exec.as_ref() }.clone();
    tracing::info!(%exec, "launch");
    let _ = std::process::Command::new("sh").arg("-c").arg(&exec).spawn();
}
