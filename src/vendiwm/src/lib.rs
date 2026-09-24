// vendiWM library root — exposes the compositor's modules.
//
// Module map (built out incrementally):
//   backends/  — winit (nested dev) + udev (real session)
//   state      — global compositor state struct
//   handlers/  — Wayland protocol handlers (compositor, xdg_shell, seat, ...)
//   input/     — keyboard/pointer/touch/gesture routing
//   layout/    — i3-style tiling tree + floating layer + drag-to-snap
//   render/    — frame composition (gles2 / pixman fallback)
//   workspace/ — per-monitor dynamic workspaces
//   ipc/       — Unix socket + JSON, sway-style request/response + events
//   config/    — KDL loader with hot-reload via notify
//   theme/     — .kdl theme manifests, palette resolution
//   bar/       — built-in status bar (workspaces, title, tray, indicators)

pub mod backends;
pub mod chrome;
pub mod config;
pub mod cursor;
pub mod input;
pub mod ipc;
pub mod layout;
pub mod render;
pub mod screencopy;
pub mod state;
pub mod text;
pub mod workspaces;
#[cfg(feature = "xwayland")]
pub mod xwayland;

/// Spawn a fire-and-forget child and wait on it from a small parked thread,
/// so it never lingers as a zombie once it exits (vendiwm used to collect
/// dead `quickshell ipc`/`notify-send` processes all session). Deliberately
/// NOT a global waitpid(-1) reaper: that would steal the exit status of
/// children vendiwm tracks itself (the screensaver's try_wait).
pub fn spawn_reaped(cmd: &mut std::process::Command) -> std::io::Result<()> {
    let mut child = cmd.spawn()?;
    let _ = std::thread::Builder::new()
        .name("reap".into())
        .stack_size(64 * 1024)
        .spawn(move || { let _ = child.wait(); });
    Ok(())
}
