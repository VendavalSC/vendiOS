// vendibar — the vendiOS status bar.
//
// GTK4 + gtk4-layer-shell. The look is deliberately minimal: no background,
// no pills, no chrome — just typography and icons floating over the
// desktop, macOS-menu-bar style. All visuals live in style.css (overridable
// at ~/.config/vendi/vendibar.css).
//
// Layout:  ◆ gem · workspaces · window title   |   clock   |   vol · net · bat

mod ipc;
mod modules;

use gtk4 as gtk;
use gtk::{gdk, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

const APP_ID:     &str = "os.vendi.vendibar";
const BAR_HEIGHT: i32  = 34;

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "vendibar=info".into()),
        )
        .init();
    tracing::info!("vendibar starting (gtk4)");

    let app = gtk::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| load_css());
    app.connect_activate(build_ui);
    // No CLI args are consumed by GTK.
    app.run_with_args::<&str>(&[])
}

/// Default theme ships in the binary; a user file at
/// ~/.config/vendi/vendibar.css replaces it wholesale.
fn load_css() {
    let provider = gtk::CssProvider::new();
    let user_css = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .map(|p| p.join("vendi/vendibar.css"));
    match user_css.filter(|p| p.exists()) {
        Some(path) => {
            tracing::info!(path = %path.display(), "loading user CSS");
            provider.load_from_path(&path);
        }
        None => provider.load_from_string(include_str!("style.css")),
    }
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("no display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn build_ui(app: &gtk::Application) {
    let window = gtk::ApplicationWindow::new(app);
    window.init_layer_shell();
    window.set_layer(Layer::Top);
    window.set_namespace(Some("vendibar"));
    window.set_keyboard_mode(KeyboardMode::None);
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Left, true);
    window.set_anchor(Edge::Right, true);
    window.auto_exclusive_zone_enable();
    window.set_default_size(-1, BAR_HEIGHT);

    let bar = gtk::CenterBox::new();
    bar.add_css_class("bar");

    // ── left: gem · workspaces · focused title ──────────────────────────────
    let left = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let logo = vendi_logo();
    let workspaces = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    workspaces.add_css_class("workspaces");
    let title = gtk::Label::new(None);
    title.add_css_class("title");
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.set_max_width_chars(48);
    left.append(&logo);
    left.append(&workspaces);
    left.append(&title);

    // ── center: clock ────────────────────────────────────────────────────────
    let clock = modules::clock();

    // ── right: music · volume · network · battery ──────────────────────────
    let right = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    right.add_css_class("status");
    right.append(&modules::music());
    right.append(&modules::volume());
    right.append(&modules::network());
    if let Some(battery) = modules::battery() {
        right.append(&battery);
    }

    bar.set_start_widget(Some(&left));
    bar.set_center_widget(Some(&clock));
    bar.set_end_widget(Some(&right));
    window.set_child(Some(&bar));
    window.present();

    // Seed workspaces so the bar isn't empty before vendiwm answers.
    render_workspaces(&workspaces, 1, &[(1, 0)]);

    // ── vendiwm IPC → UI ────────────────────────────────────────────────────
    let (tx, rx) = async_channel::unbounded::<ipc::Msg>();
    std::thread::spawn(move || ipc::listener(tx));
    glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            match msg {
                ipc::Msg::Workspaces { active, list } => {
                    render_workspaces(&workspaces, active, &list);
                }
                ipc::Msg::Title(text) => title.set_text(&text),
                ipc::Msg::Disconnected => {
                    // Compositor went away: show base state, keep retrying.
                    title.set_text("");
                }
            }
        }
    });
}

/// The vendi mark, drawn by hand: an obsidian crystal shard (think the
/// Obsidian notes logo) — a tall, slanted gem with a bright left face and a
/// deep right face, leaning right with a sharp bottom point.
fn vendi_logo() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.add_css_class("logo");
    area.set_content_width(18);
    area.set_content_height(18);
    area.set_valign(gtk::Align::Center);
    area.set_draw_func(|_, cr, w, h| {
        let (w, h) = (w as f64, h as f64);
        // Shard silhouette (clockwise):
        //   T  = top peak (off-center left)
        //   R  = right shoulder
        //   B  = bottom tip (sharp, leaning right)
        //   L  = left hip
        // Ridge runs T → B and splits the shard into two faces.
        let t = (w * 0.42, h * 0.04);
        let r = (w * 0.92, h * 0.30);
        let b = (w * 0.60, h * 0.97);
        let l = (w * 0.10, h * 0.46);

        let face = |cr: &gtk::cairo::Context, pts: &[(f64, f64)], rgb: (f64, f64, f64)| {
            cr.set_source_rgb(rgb.0, rgb.1, rgb.2);
            cr.move_to(pts[0].0, pts[0].1);
            for p in &pts[1..] { cr.line_to(p.0, p.1); }
            cr.close_path();
            let _ = cr.fill();
        };
        // Left face — bright lavender, catches the light.
        face(cr, &[t, b, l], (0.804, 0.690, 0.992));
        // Right face — deep obsidian purple.
        face(cr, &[t, r, b], (0.467, 0.310, 0.745));
        // Ridge highlight — a hairline along the T→B edge.
        cr.set_source_rgba(0.93, 0.88, 1.0, 0.85);
        cr.set_line_width(0.9);
        cr.move_to(t.0, t.1);
        cr.line_to(b.0, b.1);
        let _ = cr.stroke();
    });
    area
}

/// Rebuild the workspace indicator row. Numbers only — the active one is
/// highlighted purely via CSS, occupied ones are brighter than empty ones.
fn render_workspaces(container: &gtk::Box, active: u32, list: &[(u32, usize)]) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
    for &(id, windows) in list {
        let button = gtk::Button::with_label(&id.to_string());
        button.add_css_class("ws");
        if id == active   { button.add_css_class("active"); }
        if windows > 0    { button.add_css_class("occupied"); }
        button.connect_clicked(move |_| ipc::switch_workspace(id));
        container.append(&button);
    }
}
