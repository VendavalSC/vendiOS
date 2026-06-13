// vendi-ctl — CLI for talking to vendiwm's IPC socket.
//
// Discovery: $VENDIWM_SOCK if set, else $XDG_RUNTIME_DIR/vendiwm-1.ipc.sock.
//
// Examples:
//   vendi-ctl spawn alacritty
//   vendi-ctl list-windows
//   vendi-ctl split vertical
//   vendi-ctl subscribe window
//
// Prints JSON responses directly. Streams events line-by-line on subscribe.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        print_usage();
        std::process::exit(2);
    }

    match args[0].as_str() {
        "help" | "--help" | "-h" => { print_usage(); Ok(()) }
        "spawn"            => spawn_cmd(&args[1..]),
        "focus"            => focus_cmd(&args[1..]),
        "close"            => close_cmd(&args[1..]),
        "list-windows"     => ipc_call(json!({"cmd": "list-windows"})),
        "list-workspaces"  => ipc_call(json!({"cmd": "list-workspaces"})),
        "lock"             => lock_cmd(),
        "reload"           => ipc_call(json!({"cmd": "reload-config"})),
        "workspace"        => workspace_cmd(&args[1..]),
        "split"            => split_cmd(&args[1..]),
        "move"             => move_cmd(&args[1..]),
        "subscribe"        => subscribe_cmd(&args[1..]),
        "bar"              => bar_cmd(&args[1..]),
        "wallpaper"        => wallpaper_cmd(&args[1..]),
        "palette"          => palette_cmd(&args[1..]),
        "output"           => output_cmd(&args[1..]),
        cmd => { eprintln!("unknown command: {cmd}\n"); print_usage(); std::process::exit(2); }
    }
}

fn print_usage() {
    println!(r#"vendi-ctl — control vendiwm

Usage:
  vendi-ctl spawn <argv...>             launch a command
  vendi-ctl focus <window-id>           focus a window by id
  vendi-ctl close [window-id]           close a window (default: focused)
  vendi-ctl list-windows                snapshot of windows
  vendi-ctl list-workspaces             snapshot of workspaces
  vendi-ctl lock                        lock the session (vendi-lock)
  vendi-ctl workspace <id>              switch to a workspace
  vendi-ctl split horizontal|vertical   set next-split direction
  vendi-ctl move <window-id> <ws-id>    move window to a workspace
  vendi-ctl subscribe <event>           stream events (window, workspace)
  vendi-ctl bar title                   stream focused-title JSON (waybar exec)
  vendi-ctl wallpaper <path>            set the wallpaper (persists)
  vendi-ctl wallpaper random|next       pick from ~/Pictures/Wallpapers
  vendi-ctl wallpaper list              list ~/Pictures/Wallpapers (* = active)
  vendi-ctl wallpaper default           clear back to the procedural gradient
  vendi-ctl palette [image]             print a theme palette from an image
  vendi-ctl output list                 list monitors (name, mode, scale, pos)
  vendi-ctl output scale <name> <s>     set fractional scale (1.0, 1.5, 2.0…)
  vendi-ctl output position <name> <x> <y>  place a monitor in the layout
  vendi-ctl output mode <name> <WxH[@hz]>   set resolution / refresh
  vendi-ctl output reset [name]         clear arrangement (all, or one monitor)
  vendi-ctl reload                      re-read vendiwm.kdl live (theme, binds)

Reads $VENDIWM_SOCK or falls back to $XDG_RUNTIME_DIR/vendiwm-1.ipc.sock."#);
}

fn socket_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("VENDIWM_SOCK") {
        return Ok(PathBuf::from(p));
    }
    let rt = PathBuf::from(
        std::env::var_os("XDG_RUNTIME_DIR")
            .ok_or_else(|| anyhow::anyhow!("XDG_RUNTIME_DIR not set"))?,
    );
    // The IPC socket is named after the wayland socket ("wayland-1.ipc.sock").
    if let Ok(wl) = std::env::var("WAYLAND_DISPLAY") {
        let p = rt.join(format!("{wl}.ipc.sock"));
        if p.exists() { return Ok(p); }
    }
    // Fall back to scanning for any *.ipc.sock (e.g. called from a tty).
    if let Ok(entries) = std::fs::read_dir(&rt) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".ipc.sock") { return Ok(e.path()); }
        }
    }
    Ok(rt.join("vendiwm-1.ipc.sock"))
}

fn connect() -> Result<UnixStream> {
    let path = socket_path()?;
    UnixStream::connect(&path).with_context(|| format!("connect {}", path.display()))
}

/// Send one request, print one response, exit non-zero on error response.
/// VENDI_JSON=1 (or a trailing `json` arg routed here) skips the pretty
/// printer — scripts and the quickshell launcher parse the raw line.
fn ipc_call(request: Value) -> Result<()> {
    let mut stream = connect()?;
    let mut wire = serde_json::to_vec(&request)?;
    wire.push(b'\n');
    stream.write_all(&wire)?;

    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 { break; }
        buf.extend_from_slice(&chunk[..n]);
        if buf.contains(&b'\n') { break; }
    }
    let line = buf.split(|&b| b == b'\n').next().unwrap_or(&[]);
    let resp: Value = serde_json::from_slice(line).context("parse response")?;

    if let Some(err) = resp.get("error").and_then(|v| v.as_str()) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }

    if std::env::var_os("VENDI_JSON").is_some() {
        println!("{}", serde_json::to_string(&resp)?);
        return Ok(());
    }

    // Pretty-print known shapes; fall through to raw JSON.
    if let Some(windows) = resp.get("windows").and_then(|v| v.as_array()) {
        if windows.is_empty() { println!("(no windows)"); return Ok(()); }
        for w in windows {
            let id      = w.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
            let focused = w.get("focused").and_then(|v| v.as_bool()).unwrap_or(false);
            let title   = w.get("title").and_then(|v| v.as_str()).unwrap_or("");
            println!("{}{:>5}  {}", if focused { "* " } else { "  " }, id, title);
        }
        return Ok(());
    }
    if let Some(ws) = resp.get("workspaces").and_then(|v| v.as_array()) {
        for w in ws {
            let id      = w.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
            let focused = w.get("focused").and_then(|v| v.as_bool()).unwrap_or(false);
            println!("{}{}", if focused { "* " } else { "  " }, id);
        }
        return Ok(());
    }
    if let Some(outs) = resp.get("outputs").and_then(|v| v.as_array()) {
        if outs.is_empty() { println!("(no outputs)"); return Ok(()); }
        for o in outs {
            let name = o.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let w    = o.get("width").and_then(|v| v.as_i64()).unwrap_or(0);
            let h    = o.get("height").and_then(|v| v.as_i64()).unwrap_or(0);
            let hz   = o.get("refresh").and_then(|v| v.as_i64()).unwrap_or(0);
            let sc   = o.get("scale").and_then(|v| v.as_f64()).unwrap_or(1.0);
            let x    = o.get("x").and_then(|v| v.as_i64()).unwrap_or(0);
            let y    = o.get("y").and_then(|v| v.as_i64()).unwrap_or(0);
            println!("  {name:<12}  {w}x{h}@{hz}  scale {sc}  at {x},{y}");
        }
        return Ok(());
    }
    println!("{}", serde_json::to_string(&resp)?);
    Ok(())
}

// ── output arrangement ────────────────────────────────────────────────────────
// vendi-ctl owns ~/.config/vendi/outputs.json (its own bookkeeping) and
// regenerates ~/.config/vendi/outputs.kdl (what the compositor reads) from it,
// then triggers a live reload. Keeps KDL out of the CLI's parsing path.

fn vendi_config_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .ok_or_else(|| anyhow::anyhow!("no HOME / XDG_CONFIG_HOME"))?;
    Ok(base.join("vendi"))
}

fn load_outputs_store() -> serde_json::Map<String, Value> {
    let path = match vendi_config_dir() { Ok(d) => d.join("outputs.json"), Err(_) => return Default::default() };
    std::fs::read_to_string(path).ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn write_outputs_store(store: &serde_json::Map<String, Value>) -> Result<()> {
    let dir = vendi_config_dir()?;
    std::fs::create_dir_all(&dir)?;
    // bookkeeping JSON
    let json_path = dir.join("outputs.json");
    std::fs::write(&json_path, serde_json::to_string_pretty(store)?)?;
    // regenerate the KDL the compositor reads
    let mut kdl = String::from("// Generated by `vendi-ctl output` — edit via the CLI or the Pro GUI.\n");
    for (name, cfg) in store {
        kdl.push_str(&format!("output {:?} {{\n", name));
        if let Some(s) = cfg.get("scale").and_then(|v| v.as_f64()) {
            // `{:?}` keeps the decimal point (1.0 → "1.0"); plain `{}` prints
            // "1", which knus rejects as a float and fails the whole parse.
            kdl.push_str(&format!("    scale {s:?}\n"));
        }
        if let Some(x) = cfg.get("x").and_then(|v| v.as_i64()) {
            kdl.push_str(&format!("    x {x}\n"));
        }
        if let Some(y) = cfg.get("y").and_then(|v| v.as_i64()) {
            kdl.push_str(&format!("    y {y}\n"));
        }
        if let Some(m) = cfg.get("mode").and_then(|v| v.as_str()) {
            kdl.push_str(&format!("    mode {m:?}\n"));
        }
        kdl.push_str("}\n");
    }
    std::fs::write(dir.join("outputs.kdl"), kdl)?;
    Ok(())
}

fn output_entry<'a>(store: &'a mut serde_json::Map<String, Value>, name: &str) -> &'a mut Value {
    store.entry(name.to_string()).or_insert_with(|| json!({}))
}

fn output_cmd(args: &[String]) -> Result<()> {
    match args.first().map(|s| s.as_str()) {
        None | Some("list") => return ipc_call(json!({"cmd": "list-outputs"})),
        Some("reset") => {
            let dir = vendi_config_dir()?;
            match args.get(1) {
                Some(name) => {
                    let mut store = load_outputs_store();
                    store.remove(name);
                    write_outputs_store(&store)?;
                }
                None => {
                    let _ = std::fs::remove_file(dir.join("outputs.json"));
                    let _ = std::fs::remove_file(dir.join("outputs.kdl"));
                }
            }
            return reload_after_output();
        }
        Some("scale") => {
            let name = args.get(1).ok_or_else(|| anyhow::anyhow!("output scale: <name> <s>"))?;
            let s: f64 = args.get(2).ok_or_else(|| anyhow::anyhow!("output scale: <name> <s>"))?
                .parse().context("scale must be a number")?;
            let mut store = load_outputs_store();
            output_entry(&mut store, name)["scale"] = json!(s);
            write_outputs_store(&store)?;
        }
        Some("position") => {
            let name = args.get(1).ok_or_else(|| anyhow::anyhow!("output position: <name> <x> <y>"))?;
            let x: i64 = args.get(2).ok_or_else(|| anyhow::anyhow!("output position: <name> <x> <y>"))?
                .parse().context("x must be an integer")?;
            let y: i64 = args.get(3).ok_or_else(|| anyhow::anyhow!("output position: <name> <x> <y>"))?
                .parse().context("y must be an integer")?;
            let mut store = load_outputs_store();
            let e = output_entry(&mut store, name);
            e["x"] = json!(x); e["y"] = json!(y);
            write_outputs_store(&store)?;
        }
        Some("mode") => {
            let name = args.get(1).ok_or_else(|| anyhow::anyhow!("output mode: <name> <WxH[@hz]>"))?;
            let m = args.get(2).ok_or_else(|| anyhow::anyhow!("output mode: <name> <WxH[@hz]>"))?;
            let mut store = load_outputs_store();
            output_entry(&mut store, name)["mode"] = json!(m);
            write_outputs_store(&store)?;
        }
        Some(other) => bail!("output: unknown subcommand '{other}' (list|scale|position|mode|reset)"),
    }
    reload_after_output()
}

fn reload_after_output() -> Result<()> {
    // Scale + position apply live; a mode change takes effect on reconnect.
    ipc_call(json!({"cmd": "reload-config"}))
}

// ── per-command argv parsing ─────────────────────────────────────────────────

fn spawn_cmd(args: &[String]) -> Result<()> {
    if args.is_empty() { bail!("spawn: missing command"); }
    ipc_call(json!({"cmd": "spawn", "args": args}))
}

fn focus_cmd(args: &[String]) -> Result<()> {
    let id: u32 = args.first().ok_or_else(|| anyhow::anyhow!("focus: missing window id"))?
        .parse().context("window id must be a number")?;
    ipc_call(json!({"cmd": "focus", "window": id}))
}

fn close_cmd(args: &[String]) -> Result<()> {
    let id = args.first().map(|s| s.parse::<u32>()).transpose().context("window id must be a number")?;
    let mut req = json!({"cmd": "close"});
    if let Some(id) = id { req["window"] = json!(id); }
    ipc_call(req)
}

fn workspace_cmd(args: &[String]) -> Result<()> {
    let id: u32 = args.first().ok_or_else(|| anyhow::anyhow!("workspace: missing id"))?
        .parse().context("workspace id must be a number")?;
    ipc_call(json!({"cmd": "workspace", "id": id}))
}

fn split_cmd(args: &[String]) -> Result<()> {
    let dir = args.first().ok_or_else(|| anyhow::anyhow!("split: missing direction"))?;
    if dir != "horizontal" && dir != "vertical" { bail!("split: direction must be horizontal or vertical"); }
    ipc_call(json!({"cmd": "split", "dir": dir}))
}

fn move_cmd(args: &[String]) -> Result<()> {
    if args.len() < 2 { bail!("move: usage: move <window-id> <workspace-id>"); }
    let win: u32 = args[0].parse().context("window id")?;
    let ws:  u32 = args[1].parse().context("workspace id")?;
    ipc_call(json!({"cmd": "move", "window": win, "to_workspace": ws}))
}

// ── wallpaper ────────────────────────────────────────────────────────────────

/// The wallpaper library: ~/Pictures/Wallpapers (png/jpg/jpeg/webp), sorted.
// ── palette extraction (the Dynamic theme) ──────────────────────────────────
//
// `vendi-ctl palette [image]` prints shell-eval-able KEY=hex lines filling
// the same semantic slots the static themes use. `vendi theme dynamic`
// sources them. Defaults to the active wallpaper.

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let h = h.rem_euclid(360.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (((r + m) * 255.0) as u8, ((g + m) * 255.0) as u8, ((b + m) * 255.0) as u8)
}

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h, s, l)
}

fn hex(rgb: (u8, u8, u8)) -> String {
    format!("{:02x}{:02x}{:02x}", rgb.0, rgb.1, rgb.2)
}

fn palette_cmd(args: &[String]) -> Result<()> {
    let path = match args.first() {
        Some(p) => p.clone(),
        None => wallpaper_current()
            .ok_or_else(|| anyhow::anyhow!("palette: no wallpaper set (and no image given)"))?,
    };
    let img = image::open(&path)
        .with_context(|| format!("palette: decode {path}"))?
        .thumbnail(96, 96)
        .to_rgb8();

    // Hue histogram (24 bins) over reasonably colorful, mid-lightness pixels.
    // Each bin accumulates weight plus running color sums for averaging.
    let mut bins = vec![(0.0f32, 0.0f32, 0.0f32, 0.0f32); 24]; // weight, r, g, b
    let mut hue_x = 0.0f32; // average hue as a vector (circular mean)
    let mut hue_y = 0.0f32;
    for p in img.pixels() {
        let (h, s, l) = rgb_to_hsl(p[0], p[1], p[2]);
        if s > 0.18 && (0.18..=0.85).contains(&l) {
            // Vibrancy: saturated and near mid lightness wins.
            let w = s * (1.0 - (l - 0.55).abs() * 1.6).max(0.05);
            let bin = ((h / 15.0) as usize).min(23);
            bins[bin].0 += w;
            bins[bin].1 += p[0] as f32 * w;
            bins[bin].2 += p[1] as f32 * w;
            bins[bin].3 += p[2] as f32 * w;
            let rad = h.to_radians();
            hue_x += rad.cos() * w;
            hue_y += rad.sin() * w;
        }
    }

    // Accent: the heaviest bin, normalized into a usable range.
    let best = bins.iter().enumerate().max_by(|a, b| a.1.0.total_cmp(&b.1.0)).map(|(i, _)| i);
    let (accent_h, accent_s) = match best.filter(|&i| bins[i].0 > 1.0) {
        Some(i) => {
            let (w, r, g, b) = bins[i];
            let (h, s, _) = rgb_to_hsl((r / w) as u8, (g / w) as u8, (b / w) as u8);
            (h, s.max(0.45))
        }
        // Grayscale image — fall back to a neutral lavender-white accent.
        None => (240.0, 0.10),
    };
    let accent = hsl_to_rgb(accent_h, accent_s, 0.72);

    // Theme base hue: circular mean of the image (falls back to accent hue).
    let base_h = if hue_x.abs() + hue_y.abs() > 1e-3 {
        hue_y.atan2(hue_x).to_degrees().rem_euclid(360.0)
    } else {
        accent_h
    };

    let mut out: Vec<(&str, String)> = Vec::new();
    out.push(("A", hex(accent)));
    out.push(("base",     hex(hsl_to_rgb(base_h, 0.16, 0.12))));
    out.push(("mantle",   hex(hsl_to_rgb(base_h, 0.16, 0.09))));
    out.push(("crust",    hex(hsl_to_rgb(base_h, 0.16, 0.06))));
    out.push(("text",     hex(hsl_to_rgb(accent_h, 0.28, 0.88))));
    out.push(("subtext1", hex(hsl_to_rgb(accent_h, 0.20, 0.78))));
    out.push(("subtext0", hex(hsl_to_rgb(accent_h, 0.14, 0.66))));
    out.push(("overlay1", hex(hsl_to_rgb(base_h, 0.10, 0.50))));
    out.push(("surface2", hex(hsl_to_rgb(base_h, 0.13, 0.34))));
    out.push(("surface1", hex(hsl_to_rgb(base_h, 0.14, 0.26))));
    out.push(("surface0", hex(hsl_to_rgb(base_h, 0.15, 0.19))));

    // ANSI-ish slots: nearest sufficiently-weighted bin to each hue target;
    // synthesized from the target hue when the image has nothing there.
    for (name, target) in [
        ("blue", 220.0f32), ("teal", 175.0), ("green", 120.0),
        ("yellow", 50.0), ("red", 5.0), ("pink", 325.0),
    ] {
        let tbin = ((target / 15.0) as usize).min(23);
        // Look in the target bin ± 1 for real image color.
        let found = [tbin, (tbin + 1) % 24, (tbin + 23) % 24].iter()
            .filter(|&&i| bins[i].0 > 2.0)
            .max_by(|&&a, &&b| bins[a].0.total_cmp(&bins[b].0))
            .copied();
        let rgb = match found {
            Some(i) => {
                let (w, r, g, b) = bins[i];
                let (h, s, _) = rgb_to_hsl((r / w) as u8, (g / w) as u8, (b / w) as u8);
                hsl_to_rgb(h, s.max(0.40), 0.70)
            }
            None => hsl_to_rgb(target, 0.50, 0.70),
        };
        out.push((name, hex(rgb)));
    }

    for (k, v) in out {
        println!("{k}={v}");
    }
    Ok(())
}

/// Prefer the quickshell lock screen (vendilock, ext-session-lock); fall
/// back to the compositor-native vendi-lock when quickshell is missing.
fn lock_cmd() -> Result<()> {
    let has_vendilock = std::process::Command::new("sh")
        .arg("-c")
        .arg(r#"command -v quickshell >/dev/null && { [ -f "$HOME/.config/quickshell/vendilock/shell.qml" ] || [ -f /etc/xdg/quickshell/vendilock/shell.qml ]; }"#)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if has_vendilock {
        std::process::Command::new("quickshell")
            .args(["-c", "vendilock"])
            .spawn()
            .context("spawn vendilock")?;
        Ok(())
    } else {
        ipc_call(json!({"cmd": "lock"}))
    }
}

fn wallpaper_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME not set"))?;
    Ok(PathBuf::from(home).join("Pictures/Wallpapers"))
}

fn wallpaper_library() -> Result<Vec<PathBuf>> {
    let dir = wallpaper_dir()?;
    let mut out: Vec<PathBuf> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd.flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "webp"))
                    .unwrap_or(false)
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    Ok(out)
}

/// The active wallpaper as persisted by the compositor.
fn wallpaper_current() -> Option<String> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    let text = std::fs::read_to_string(base.join("vendi/wallpaper")).ok()?;
    let p = text.trim().to_string();
    (!p.is_empty()).then_some(p)
}

fn wallpaper_cmd(args: &[String]) -> Result<()> {
    let arg = args.first().map(String::as_str)
        .ok_or_else(|| anyhow::anyhow!("wallpaper: usage: wallpaper <path|random|next|list|default>"))?;
    let path = match arg {
        "default" | "none" => {
            return ipc_call(json!({"cmd": "wallpaper", "path": "default"}));
        }
        "list" => {
            let cur = wallpaper_current();
            for p in wallpaper_library()? {
                let mark = if Some(p.to_string_lossy().as_ref()) == cur.as_deref() { "*" } else { " " };
                println!("{mark} {}", p.display());
            }
            return Ok(());
        }
        "random" => {
            let lib = wallpaper_library()?;
            if lib.is_empty() { bail!("no images in {}", wallpaper_dir()?.display()); }
            // Avoid a rand dependency: nanos are plenty for "shuffle my desk".
            let n = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?.subsec_nanos() as usize;
            lib[n % lib.len()].clone()
        }
        "next" => {
            let lib = wallpaper_library()?;
            if lib.is_empty() { bail!("no images in {}", wallpaper_dir()?.display()); }
            let cur = wallpaper_current();
            let idx = cur.as_deref()
                .and_then(|c| lib.iter().position(|p| p.to_string_lossy() == c))
                .map(|i| (i + 1) % lib.len())
                .unwrap_or(0);
            lib[idx].clone()
        }
        p => {
            let p = PathBuf::from(p);
            p.canonicalize().with_context(|| format!("wallpaper: {}", p.display()))?
        }
    };
    ipc_call(json!({"cmd": "wallpaper", "path": path.to_string_lossy()}))?;
    // The Dynamic theme follows the wallpaper — regenerate it in the
    // background after a switch.
    let dynamic_active = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".config/vendi/theme-state"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.contains("THEME=dynamic"))
        .unwrap_or(false);
    if dynamic_active {
        let _ = std::process::Command::new("sh")
            .args(["-c", "vendi theme dynamic >/dev/null 2>&1"])
            .spawn();
    }
    Ok(())
}

// ── waybar exec adapters ─────────────────────────────────────────────────────
//
// waybar's `exec` with no `interval` runs the command once and reads JSON lines
// from stdout for the lifetime of the bar. Each line becomes the module state.
// We emit one line on startup (snapshot) then one more after every event push.

fn bar_cmd(args: &[String]) -> Result<()> {
    let what = args.first().map(String::as_str).unwrap_or("");
    match what {
        "title" => bar_title_stream(),
        _ => { eprintln!("bar: unknown sub-mode '{what}' (expected: title)"); std::process::exit(2); }
    }
}

fn bar_title_stream() -> Result<()> {
    // 1. Emit the current focused title as the first line so the bar isn't blank.
    emit_focused_title()?;
    // 2. Open a long-lived subscribe connection; re-emit on every window event.
    let mut stream = connect()?;
    let mut wire = serde_json::to_vec(&json!({"cmd": "subscribe", "events": ["window"]}))?;
    wire.push(b'\n');
    stream.write_all(&wire)?;
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let _ = line.context("read event")?;
        emit_focused_title()?;
    }
    Ok(())
}

fn emit_focused_title() -> Result<()> {
    let mut stream = connect()?;
    let mut wire = serde_json::to_vec(&json!({"cmd": "list-windows"}))?;
    wire.push(b'\n');
    stream.write_all(&wire)?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 { break; }
        buf.extend_from_slice(&chunk[..n]);
        if buf.contains(&b'\n') { break; }
    }
    let line = buf.split(|&b| b == b'\n').next().unwrap_or(&[]);
    let resp: Value = serde_json::from_slice(line).unwrap_or(Value::Null);
    let title = resp.get("windows")
        .and_then(|v| v.as_array())
        .and_then(|ws| ws.iter().find(|w| w.get("focused").and_then(|f| f.as_bool()).unwrap_or(false)))
        .and_then(|w| w.get("title").and_then(|t| t.as_str()))
        .unwrap_or("")
        .to_string();
    let out = json!({ "text": title, "tooltip": title, "class": if title.is_empty() { "idle" } else { "active" } });
    println!("{}", out);
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    Ok(())
}

/// Subscribe: send the request, then stream events line-by-line forever.
fn subscribe_cmd(args: &[String]) -> Result<()> {
    if args.is_empty() { bail!("subscribe: missing event kind"); }
    let mut stream = connect()?;
    let req = json!({"cmd": "subscribe", "events": args});
    let mut wire = serde_json::to_vec(&req)?;
    wire.push(b'\n');
    stream.write_all(&wire)?;

    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line.context("read event")?;
        if line.is_empty() { continue; }
        println!("{line}");
    }
    Ok(())
}
