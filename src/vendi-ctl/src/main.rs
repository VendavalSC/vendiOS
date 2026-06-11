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
        "lock"             => ipc_call(json!({"cmd": "lock"})),
        "workspace"        => workspace_cmd(&args[1..]),
        "split"            => split_cmd(&args[1..]),
        "move"             => move_cmd(&args[1..]),
        "subscribe"        => subscribe_cmd(&args[1..]),
        "bar"              => bar_cmd(&args[1..]),
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
    println!("{}", serde_json::to_string(&resp)?);
    Ok(())
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
