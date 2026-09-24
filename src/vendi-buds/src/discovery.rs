//! Finding the currently-connected Bluetooth audio device. Shells out to
//! `bluetoothctl` rather than talking D-Bus directly — matches how the rest
//! of vendiOS's CLIs already lean on existing tools (nmcli, powerprofilesctl)
//! instead of pulling in a D-Bus client library.

use std::process::Command;

#[derive(Debug, Clone)]
pub struct ConnectedDevice {
    pub mac: String,
    pub name: String,
}

/// The first currently-connected device, if any. AirPods (and most BT
/// headphones) show up here the same way any other connected device would —
/// there's no reliable "is this audio" filter from bluetoothctl alone, so
/// vendi-buds just tries the AAP handshake on whatever's connected and falls
/// back to "generic" if that doesn't pan out (see `main.rs`).
pub fn connected_device() -> Option<ConnectedDevice> {
    let out = Command::new("bluetoothctl")
        .args(["devices", "Connected"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // "Device AA:BB:CC:DD:EE:FF Some Name Here"
    let line = text.lines().next()?;
    let mut parts = line.splitn(3, ' ');
    parts.next()?; // "Device"
    let mac = parts.next()?.to_string();
    let name = parts.next().unwrap_or("Unknown").to_string();
    Some(ConnectedDevice { mac, name })
}
