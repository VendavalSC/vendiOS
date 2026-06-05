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
pub mod input;
pub mod ipc;
pub mod layout;
pub mod state;
