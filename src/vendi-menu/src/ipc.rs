// vendiwm IPC client — one-shot requests only.
//
// Socket: $VENDIWM_SOCK, falling back to
// $XDG_RUNTIME_DIR/$WAYLAND_DISPLAY.ipc.sock.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};

fn socket_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("VENDIWM_SOCK") {
        return Some(PathBuf::from(p));
    }
    let run  = std::env::var_os("XDG_RUNTIME_DIR")?;
    let disp = std::env::var_os("WAYLAND_DISPLAY")?;
    Some(PathBuf::from(run).join(format!("{}.ipc.sock", disp.to_string_lossy())))
}

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

/// All active keybinds as (chord, action), defaults merged with user overrides.
pub fn list_binds() -> Vec<(String, String)> {
    match request(&json!({"cmd": "list-binds"})) {
        Ok(v) => v["binds"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|b| {
                        Some((
                            b["chord"].as_str()?.to_string(),
                            b["action"].as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Err(e) => {
            tracing::warn!(?e, "list-binds failed");
            Vec::new()
        }
    }
}
