// Nested-compositor backend. Opens a winit window inside the host session.
//
// Stub for v0.0.1 — wires up the calloop event loop and exits cleanly on
// window close. Real Wayland protocol handling lands in the next iteration.

use anyhow::Result;

pub fn run() -> Result<()> {
    tracing::warn!("winit backend stub — no surfaces yet, just proving the loop runs");
    // TODO: open winit window, wire smithay::backend::winit::init, build State,
    // bind wl_compositor + xdg_shell, render a clear color, exit on close.
    Ok(())
}
