// Actions mode — `vendi-menu actions`.
//
// A nested system menu, no search bar: apps, keybinds, capture, theme,
// wallpaper, settings, install, about, power. Submenus slide in and out of
// a GtkStack. Arrow keys + Enter navigate, digits fire rows directly,
// Escape/Left/Backspace go back (or close at the root). Mouse works.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4 as gtk;
use gtk::{gdk, glib, prelude::*};
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};

const WIDTH: i32 = 380;
/// Spawn a command in a floating terminal (vendiwm floats app_id vendi-float).
const FLOAT_TERM: &str = "alacritty --class vendi-float -e";

pub enum Entry {
    Menu(Rc<MenuDef>),
    Sh(String),
    Func(Box<dyn Fn()>),
}

pub struct ItemDef {
    pub glyph: &'static str,
    pub label: String,
    pub entry: Entry,
}

pub struct MenuDef {
    pub title: &'static str,
    pub items: Vec<ItemDef>,
}

fn sh(glyph: &'static str, label: &str, cmd: impl Into<String>) -> ItemDef {
    ItemDef { glyph, label: label.into(), entry: Entry::Sh(cmd.into()) }
}

fn submenu(glyph: &'static str, label: &str, menu: MenuDef) -> ItemDef {
    ItemDef { glyph, label: label.into(), entry: Entry::Menu(Rc::new(menu)) }
}

/// Run `cmd` inside a floating terminal, holding the window until a key.
fn term(cmd: &str) -> String {
    format!("{FLOAT_TERM} sh -c '{cmd}; printf \"\\n  done — any key closes \"; read -rsn1'")
}

fn root_menu() -> Rc<MenuDef> {
    let capture = MenuDef {
        title: "Capture",
        items: vec![
            sh("\u{f0c4e}", "Region to clipboard", r#"sh -c 'grim -g "$(slurp)" - | wl-copy'"#),
            sh("\u{f1077}", "Region to file",      r#"sh -c 'mkdir -p ~/Pictures && grim -g "$(slurp)" ~/Pictures/screenshot-$(date +%s).png'"#),
            sh("\u{f0e51}", "Screen to file",      r#"sh -c 'mkdir -p ~/Pictures && grim ~/Pictures/screenshot-$(date +%s).png'"#),
        ],
    };

    let settings = MenuDef {
        title: "Settings",
        items: vec![
            sh("\u{f035b}", "Bar: minimal", "sh -c 'mkdir -p ~/.config/vendi; echo classic > ~/.config/vendi/bar; pkill -x vendiwm'"),
            sh("\u{f035c}", "Bar: pro",     "sh -c 'mkdir -p ~/.config/vendi; echo pro > ~/.config/vendi/bar; pkill -x vendiwm'"),
            sh("\u{f0493}", "WM config",  format!("{FLOAT_TERM} sh -c 'mkdir -p ~/.config/vendi && ${{EDITOR:-vim}} ~/.config/vendi/vendiwm.kdl'")),
            sh("\u{f035b}", "Bar style",  format!("{FLOAT_TERM} sh -c 'mkdir -p ~/.config/vendi && ${{EDITOR:-vim}} ~/.config/vendi/vendibar.css'")),
            sh("\u{f0450}", "Reload session", "pkill -x vendiwm"),
        ],
    };

    // The vendi TUIs are fzf loops that exit on Escape — no hold needed.
    let connect = MenuDef {
        title: "Connect",
        items: vec![
            sh("\u{f05a9}", "Wi-Fi",         format!("{FLOAT_TERM} vendi wifi")),
            sh("\u{f00af}", "Bluetooth",     format!("{FLOAT_TERM} vendi bt")),
            sh("\u{f057e}", "Audio output",  format!("{FLOAT_TERM} vendi audio")),
            sh("\u{f0210}", "Power profile", format!("{FLOAT_TERM} vendi power")),
        ],
    };

    let install = MenuDef {
        title: "Install",
        items: vec![
            sh("\u{f0419}", "Install package", term(r#"pacman -Slq | fzf --multi --prompt="install> " --preview "pacman -Si {}" | xargs -ro sudo pacman -S"#)),
            sh("\u{f0376}", "Remove package",  term(r#"pacman -Qq | fzf --multi --prompt="remove> " --preview "pacman -Qi {}" | xargs -ro sudo pacman -Rns"#)),
            sh("\u{f06b0}", "Update system",   term("sudo vendi update")),
        ],
    };

    let power = MenuDef {
        title: "Power",
        items: vec![
            sh("\u{f033e}", "Lock",             "vendi-ctl lock"),
            sh("\u{f04b2}", "Suspend",          "systemctl suspend"),
            sh("\u{f0450}", "Relaunch session", "pkill -x vendiwm"),
            sh("\u{f0709}", "Restart",          "systemctl reboot"),
            sh("\u{f0425}", "Shutdown",         "systemctl poweroff"),
        ],
    };

    Rc::new(MenuDef {
        title: "vendiOS",
        items: vec![
            sh("\u{f0451}", "Apps",     "vendi-menu"),
            sh("\u{f030c}", "Keybinds", "vendi-menu keys"),
            submenu("\u{f0100}", "Capture",   capture),
            submenu("\u{f03d8}", "Theme",     theme_menu()),
            submenu("\u{f0e09}", "Wallpaper", wallpaper_menu()),
            submenu("\u{f0493}", "Settings",  settings),
            submenu("\u{f05a9}", "Connect",   connect),
            submenu("\u{f0419}", "Install",   install),
            sh("\u{f02fd}", "About", term("vendi fetch")),
            submenu("\u{f0425}", "Power", power),
        ],
    })
}

// ── theme / wallpaper (write the theme block, relaunch the compositor) ───────

// Theme switching delegates to `vendi theme`, which recolors EVERYTHING
// (vendiwm, bar, menu, alacritty, swaylock), then relaunches the session.
fn theme_menu() -> MenuDef {
    // Big global themes — one pick, full look. Matches `vendi theme list`.
    const THEMES: &[(&str, &str, &str)] = &[
        ("\u{f0405}", "Mocha — signature dark",       "mocha"),
        ("\u{f0599}", "Latte — light",                "latte"),
        ("\u{f1b35}", "Gruvbox — warm retro",         "gruvbox"),
        ("\u{f0765}", "Mono — black & white",         "mono"),
        ("\u{f08c7}", "Think — ThinkPad black & red", "think"),
    ];
    MenuDef {
        title: "Theme",
        items: THEMES.iter().map(|&(glyph, label, id)| {
            sh(glyph, label, format!("sh -c 'vendi theme {id} >/dev/null; pkill -x vendiwm'"))
        }).collect(),
    }
}

fn wallpaper_menu() -> MenuDef {
    let mut items = vec![ItemDef {
        glyph: "\u{f06e8}",
        label: "Default gradient".into(),
        entry: Entry::Func(Box::new(|| {
            set_theme_key("wallpaper", None);
            relaunch();
        })),
    }];

    let mut dirs = vec![PathBuf::from("/usr/share/vendi/wallpapers")];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.insert(0, PathBuf::from(home).join("Pictures/Wallpapers"));
    }
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                matches!(
                    p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
                    Some("png" | "jpg" | "jpeg")
                )
            })
            .collect();
        files.sort();
        for path in files {
            let label = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let path_str = path.to_string_lossy().into_owned();
            items.push(ItemDef {
                glyph: "\u{f0e09}",
                label,
                entry: Entry::Func(Box::new(move || {
                    set_theme_key("wallpaper", Some(&format!("\"{path_str}\"")));
                    relaunch();
                })),
            });
        }
    }
    MenuDef { title: "Wallpaper", items }
}

pub(crate) fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("vendi").join("vendiwm.kdl")
}

/// Set (or remove, value=None) one key in the `theme { }` block of
/// vendiwm.kdl, preserving everything else in the file. The compositor merges
/// user config over its defaults, so a theme-only file keeps all binds.
pub(crate) fn set_theme_key(key: &str, value: Option<&str>) {
    let path = config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<String> = text.lines().map(String::from).collect();

    let start = lines.iter().position(|l| {
        let t = l.trim_start();
        t.starts_with("theme") && t.contains('{')
    });

    match start {
        Some(start) => {
            let end = lines[start + 1..].iter()
                .position(|l| l.trim() == "}")
                .map(|i| i + start + 1);
            let Some(end) = end else { return };
            let key_line = (start + 1..end)
                .find(|&i| lines[i].trim_start().split_whitespace().next() == Some(key));
            match (key_line, value) {
                (Some(i), Some(v)) => lines[i] = format!("    {key} {v}"),
                (Some(i), None)    => { lines.remove(i); }
                (None, Some(v))    => lines.insert(end, format!("    {key} {v}")),
                (None, None)       => {}
            }
        }
        None => {
            if let Some(v) = value {
                if !lines.is_empty() { lines.push(String::new()); }
                lines.push("theme {".into());
                lines.push(format!("    {key} {v}"));
                lines.push("}".into());
            }
        }
    }

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, lines.join("\n") + "\n") {
        tracing::error!(?e, "write vendiwm.kdl failed");
    }
}

pub(crate) fn relaunch() {
    let _ = std::process::Command::new("pkill").args(["-x", "vendiwm"]).spawn();
}

// ── power profile menu (`vendi-menu power`, the bar battery click) ──────────

/// One-level menu listing powerprofilesctl profiles; the active one is marked.
fn power_profile_menu() -> Rc<MenuDef> {
    let out = std::process::Command::new("powerprofilesctl").arg("list").output();
    let text = out.ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    // Lines like "* balanced:" / "  power-saver:" head each profile block.
    let mut items: Vec<ItemDef> = text.lines()
        .filter_map(|l| {
            let active = l.starts_with("* ");
            let name = l.trim_start_matches("* ").trim();
            let name = name.strip_suffix(':')?;
            if name.contains(' ') { return None; }
            let glyph = match name {
                "performance" => "\u{f04c5}",   // 󰓅
                "power-saver" => "\u{f032a}",   // 󰌪
                _             => "\u{f0f85}",   // 󰾅 balanced
            };
            let label = if active { format!("{name} — active") } else { name.to_string() };
            Some(sh(glyph, &label, format!("powerprofilesctl set {name}")))
        })
        .collect();
    if items.is_empty() {
        items.push(sh("\u{f0210}", "power profiles unavailable", "true"));
    }
    Rc::new(MenuDef { title: "Power profile", items })
}

pub fn build_power_ui(app: &gtk::Application) {
    build_menu_ui(app, power_profile_menu());
}

// ── UI ────────────────────────────────────────────────────────────────────────

type Pages = Rc<RefCell<HashMap<String, (Rc<MenuDef>, gtk::ListBox)>>>;

pub fn build_ui(app: &gtk::Application) {
    build_menu_ui(app, root_menu());
}

fn build_menu_ui(app: &gtk::Application, menu: Rc<MenuDef>) {
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
    window.set_default_width(WIDTH);
    window.add_css_class("menu");

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("menu-root");
    root.add_css_class("actions-root");

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::SlideLeft);
    stack.set_transition_duration(220);
    stack.set_hhomogeneous(true);
    stack.set_vhomogeneous(false);
    stack.set_interpolate_size(true);

    let pages: Pages = Rc::new(RefCell::new(HashMap::new()));
    let nav: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(vec!["root".into()]));

    build_page(app, &stack, &pages, &nav, &window, &menu, "root");
    stack.set_visible_child_name("root");

    root.append(&stack);
    window.set_child(Some(&root));

    // Keyboard: arrows move, Enter/Right descend or fire, Escape/Left/
    // Backspace go back (Escape at the root closes), digits fire rows.
    let keys = gtk::EventControllerKey::new();
    {
        let (app, stack, pages, nav, window) =
            (app.clone(), stack.clone(), pages.clone(), nav.clone(), window.clone());
        keys.connect_key_pressed(move |_, key, _, _| {
            let current = nav.borrow().last().cloned().unwrap_or_default();
            let Some((_, list)) = pages.borrow().get(&current).map(|(m, l)| (m.clone(), l.clone())) else {
                return glib::Propagation::Proceed;
            };
            match key {
                gdk::Key::Escape | gdk::Key::Left | gdk::Key::BackSpace => {
                    if nav.borrow().len() > 1 {
                        go_back(&stack, &nav, &pages, &window);
                    } else {
                        app.quit();
                    }
                    glib::Propagation::Stop
                }
                gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::Right => {
                    if let Some(row) = list.selected_row() {
                        activate(&app, &stack, &pages, &nav, &window, row.index());
                    }
                    glib::Propagation::Stop
                }
                gdk::Key::Down | gdk::Key::Up | gdk::Key::Tab => {
                    let len = pages.borrow().get(&current).map(|(m, _)| m.items.len()).unwrap_or(0) as i32;
                    if len > 0 {
                        let delta = if key == gdk::Key::Up { -1 } else { 1 };
                        let cur = list.selected_row().map(|r| r.index()).unwrap_or(0);
                        if let Some(row) = list.row_at_index((cur + delta).rem_euclid(len)) {
                            list.select_row(Some(&row));
                        }
                    }
                    glib::Propagation::Stop
                }
                k => {
                    if let Some(d) = k.to_unicode().and_then(|c| c.to_digit(10)) {
                        let len = pages.borrow().get(&current).map(|(m, _)| m.items.len()).unwrap_or(0);
                        if d >= 1 && (d as usize) <= len {
                            activate(&app, &stack, &pages, &nav, &window, d as i32 - 1);
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

/// Build one menu page (and, recursively, its submenu pages) into the stack.
fn build_page(
    app: &gtk::Application,
    stack: &gtk::Stack,
    pages: &Pages,
    nav: &Rc<RefCell<Vec<String>>>,
    window: &gtk::ApplicationWindow,
    menu: &Rc<MenuDef>,
    path: &str,
) {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);

    if path != "root" {
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.add_css_class("page-title");
        let chevron = gtk::Label::new(Some("\u{f0141}"));
        chevron.add_css_class("page-back");
        let title = gtk::Label::new(Some(menu.title));
        title.add_css_class("page-name");
        title.set_xalign(0.0);
        title.set_hexpand(true);
        header.append(&chevron);
        header.append(&title);
        page.append(&header);
    }

    let list = gtk::ListBox::new();
    list.add_css_class("results");
    list.add_css_class("actions");
    list.set_selection_mode(gtk::SelectionMode::Browse);

    for (i, item) in menu.items.iter().enumerate() {
        let row = gtk::ListBoxRow::new();
        row.add_css_class("hit");
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        let glyph = gtk::Label::new(Some(item.glyph));
        glyph.add_css_class("action-glyph");
        let label = gtk::Label::new(Some(&item.label));
        label.add_css_class("hit-name");
        label.set_xalign(0.0);
        label.set_hexpand(true);
        line.append(&glyph);
        line.append(&label);
        if matches!(item.entry, Entry::Menu(_)) {
            let arrow = gtk::Label::new(Some("\u{f0142}"));
            arrow.add_css_class("action-index");
            line.append(&arrow);
        } else {
            let index = gtk::Label::new(Some(&(i + 1).to_string()));
            index.add_css_class("action-index");
            line.append(&index);
        }
        row.set_child(Some(&line));
        list.append(&row);
    }
    if let Some(first) = list.row_at_index(0) {
        list.select_row(Some(&first));
    }

    {
        let (app, stack, pages, nav, window) =
            (app.clone(), stack.clone(), pages.clone(), nav.clone(), window.clone());
        list.connect_row_activated(move |_, row| {
            activate(&app, &stack, &pages, &nav, &window, row.index());
        });
    }

    page.append(&list);
    stack.add_named(&page, Some(path));
    pages.borrow_mut().insert(path.to_string(), (menu.clone(), list));

    for (i, item) in menu.items.iter().enumerate() {
        if let Entry::Menu(sub) = &item.entry {
            build_page(app, stack, pages, nav, window, sub, &format!("{path}/{i}"));
        }
    }
}

fn activate(
    app: &gtk::Application,
    stack: &gtk::Stack,
    pages: &Pages,
    nav: &Rc<RefCell<Vec<String>>>,
    window: &gtk::ApplicationWindow,
    idx: i32,
) {
    let current = nav.borrow().last().cloned().unwrap_or_default();
    let Some(menu) = pages.borrow().get(&current).map(|(m, _)| m.clone()) else { return };
    let Some(item) = menu.items.get(idx as usize) else { return };
    match &item.entry {
        Entry::Menu(_) => {
            let child = format!("{current}/{idx}");
            stack.set_transition_type(gtk::StackTransitionType::SlideLeft);
            stack.set_visible_child_name(&child);
            nav.borrow_mut().push(child.clone());
            if let Some((_, list)) = pages.borrow().get(&child) {
                if let Some(first) = list.row_at_index(0) {
                    list.select_row(Some(&first));
                }
                list.grab_focus();
            }
            shrink_later(window);
        }
        Entry::Sh(cmd) => {
            tracing::info!(%cmd, "action");
            let _ = std::process::Command::new("sh").arg("-c").arg(cmd).spawn();
            app.quit();
        }
        Entry::Func(f) => {
            f();
            app.quit();
        }
    }
}

fn go_back(
    stack: &gtk::Stack,
    nav: &Rc<RefCell<Vec<String>>>,
    pages: &Pages,
    window: &gtk::ApplicationWindow,
) {
    nav.borrow_mut().pop();
    let parent = nav.borrow().last().cloned().unwrap_or_else(|| "root".into());
    stack.set_transition_type(gtk::StackTransitionType::SlideRight);
    stack.set_visible_child_name(&parent);
    if let Some((_, list)) = pages.borrow().get(&parent) {
        list.grab_focus();
    }
    shrink_later(window);
}

/// Layer-shell windows never shrink on their own — after the page slide
/// finishes, reset the default size so the surface re-measures.
fn shrink_later(window: &gtk::ApplicationWindow) {
    let window = window.clone();
    glib::timeout_add_local_once(std::time::Duration::from_millis(240), move || {
        window.set_default_size(WIDTH, -1);
    });
}
