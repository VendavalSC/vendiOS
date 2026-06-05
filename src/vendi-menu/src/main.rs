// vendi-menu — launcher / runner / power menu.
//
// Stub for v0.0.1. The launcher will be a Wayland client (layer-shell surface)
// that talks to vendiwm over $VENDIWM_SOCK for window-switcher mode.

use anyhow::Result;

fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    tracing::info!("vendi-menu stub — UI lands once vendiwm renders something");
    Ok(())
}
