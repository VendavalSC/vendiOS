//! Tiny control socket the CLI (`vendi buds noise <mode>`) talks to. Status
//! reads don't go through here — they read `buds-state.json` directly, same
//! as the bar does. This is write-only: one command per line, no reply.

use crate::aap::NoiseMode;
use anyhow::{Context, Result};
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

pub enum Command {
    SetNoiseMode(NoiseMode),
}

pub fn socket_path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").unwrap_or_else(|| "/tmp".into());
    PathBuf::from(runtime).join("vendi-buds.sock")
}

/// Runs forever on its own thread, forwarding parsed commands to `tx`.
pub fn serve(tx: Sender<Command>) -> Result<()> {
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).with_context(|| format!("bind {}", path.display()))?;
    for stream in listener.incoming().flatten() {
        let reader = BufReader::new(stream);
        for line in reader.lines().map_while(Result::ok) {
            let mut parts = line.trim().splitn(2, ' ');
            match (parts.next(), parts.next()) {
                (Some("noise"), Some(mode)) => {
                    if let Some(m) = NoiseMode::parse(mode) {
                        let _ = tx.send(Command::SetNoiseMode(m));
                    } else {
                        tracing::warn!(mode, "unknown noise mode");
                    }
                }
                _ => tracing::warn!(%line, "unrecognized control command"),
            }
        }
    }
    Ok(())
}
