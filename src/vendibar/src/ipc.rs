// vendiwm IPC client.
//
// Socket: $VENDIWM_SOCK, falling back to
// $XDG_RUNTIME_DIR/$WAYLAND_DISPLAY.ipc.sock (vendiwm binds its IPC socket
// adjacent to its wayland socket).
//
// One long-lived subscribe connection feeds events to the UI thread through
// an async-channel; short-lived connections handle one-shot requests
// (snapshots, workspace switching on click).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};

#[derive(Debug)]
pub enum Msg {
    Workspaces { active: u32, list: Vec<(u32, usize)> },
    Title(String),
    Disconnected,
}

fn socket_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("VENDIWM_SOCK") {
        return Some(PathBuf::from(p));
    }
    let run  = std::env::var_os("XDG_RUNTIME_DIR")?;
    let disp = std::env::var_os("WAYLAND_DISPLAY")?;
    Some(PathBuf::from(run).join(format!("{}.ipc.sock", disp.to_string_lossy())))
}

/// One-shot request/response.
fn request(req: &Value) -> Result<Value> {
    let path = socket_path().context("no IPC socket path")?;
    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!("connect {}", path.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut wire = serde_json::to_vec(req)?;
    wire.push(b'\n');
    stream.write_all(&wire)?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    Ok(serde_json::from_str(&line)?)
}

/// Fire-and-forget workspace switch (bar click).
pub fn switch_workspace(id: u32) {
    std::thread::spawn(move || {
        if let Err(e) = request(&json!({"cmd": "workspace", "id": id})) {
            tracing::warn!(?e, id, "workspace switch failed");
        }
    });
}

/// Background loop: snapshot + subscribe, retrying forever with backoff.
pub fn listener(tx: async_channel::Sender<Msg>) {
    loop {
        if let Err(e) = run_once(&tx) {
            tracing::debug!(?e, "ipc cycle ended");
        }
        if tx.send_blocking(Msg::Disconnected).is_err() {
            return;   // UI is gone
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn run_once(tx: &async_channel::Sender<Msg>) -> Result<()> {
    // Snapshots first so the bar fills in immediately.
    if let Ok(v) = request(&json!({"cmd": "list-workspaces"})) {
        if let Some(msg) = parse_workspaces(&v) {
            let _ = tx.send_blocking(msg);
        }
    }
    if let Ok(v) = request(&json!({"cmd": "list-windows"})) {
        let title = v.get("windows")
            .and_then(|w| w.as_array())
            .and_then(|ws| ws.iter().find(|w| {
                w.get("focused").and_then(Value::as_bool).unwrap_or(false)
            }))
            .and_then(|w| w.get("title").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        let _ = tx.send_blocking(Msg::Title(title));
    }

    // Long-lived event stream.
    let path = socket_path().context("no IPC socket path")?;
    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!("connect {}", path.display()))?;
    let mut wire = serde_json::to_vec(&json!({
        "cmd": "subscribe", "events": ["window", "workspace"],
    }))?;
    wire.push(b'\n');
    stream.write_all(&wire)?;

    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line.context("read event")?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        match v.get("event").and_then(Value::as_str) {
            Some("workspaces-changed") => {
                if let Some(msg) = parse_workspaces(&v) {
                    tx.send_blocking(msg)?;
                }
            }
            Some("window-focused") => {
                let title = v.get("title").and_then(Value::as_str).unwrap_or("");
                tx.send_blocking(Msg::Title(title.to_string()))?;
            }
            Some("window-title") => {
                if v.get("focused").and_then(Value::as_bool).unwrap_or(false) {
                    let title = v.get("title").and_then(Value::as_str).unwrap_or("");
                    tx.send_blocking(Msg::Title(title.to_string()))?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Both the list-workspaces response and the workspaces-changed event carry
/// a `workspaces` array of {id, focused, windows}.
fn parse_workspaces(v: &Value) -> Option<Msg> {
    let arr = v.get("workspaces")?.as_array()?;
    let mut active = 1;
    let mut list = Vec::with_capacity(arr.len());
    for ws in arr {
        let id      = ws.get("id")?.as_u64()? as u32;
        let focused = ws.get("focused").and_then(Value::as_bool).unwrap_or(false);
        let windows = ws.get("windows").and_then(Value::as_u64).unwrap_or(0) as usize;
        if focused { active = id; }
        list.push((id, windows));
    }
    Some(Msg::Workspaces { active, list })
}
