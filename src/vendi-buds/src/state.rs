//! Shared daemon state — written to a small JSON file the bar polls (same
//! pattern as `~/.config/vendi/wallpaper-dynamic`, `theme-state`, etc.), so
//! vendibar-pro doesn't need its own IPC client for something this simple.

use crate::aap::{Battery, NoiseMode};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Airpods,
    /// Connected over standard Bluetooth but didn't answer the AAP handshake
    /// (any non-Apple headset) — shown with a generic icon, no controls.
    Generic,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct State {
    pub connected: bool,
    pub name: String,
    pub kind: Kind,
    pub noise_mode: Option<NoiseMode>,
    pub battery: Battery,
}

impl State {
    pub fn disconnected() -> Self {
        Self {
            connected: false,
            name: String::new(),
            kind: Kind::Generic,
            noise_mode: None,
            battery: Battery::default(),
        }
    }

    pub fn path() -> PathBuf {
        let home = std::env::var_os("HOME").expect("HOME must be set");
        PathBuf::from(home).join(".config/vendi/buds-state.json")
    }

    /// Atomic tmp+rename write — same convention as the other `~/.config/vendi/*`
    /// state files, so a FileView watcher never observes a half-written file.
    pub fn write(&self) -> anyhow::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(serde_json::to_string(self)?.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}
