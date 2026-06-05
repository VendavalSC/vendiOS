// vendiWM — definitive Wayland compositor for vendiOS.
//
// This binary starts the compositor. The actual logic lives in lib.rs and
// the modules below it.

use anyhow::Result;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    init_tracing();

    tracing::info!("vendiwm starting");

    let backend = pick_backend();
    tracing::info!(?backend, "selected backend");

    match backend {
        Backend::Winit => {
            #[cfg(feature = "winit")]
            { vendiwm::backends::winit::run()?; }
            #[cfg(not(feature = "winit"))]
            anyhow::bail!("winit backend not compiled in — rebuild with --features winit");
        }
        Backend::Udev => {
            #[cfg(feature = "udev")]
            { vendiwm::backends::udev::run()?; }
            #[cfg(not(feature = "udev"))]
            anyhow::bail!("udev backend not compiled in — rebuild with --features udev");
        }
    }

    tracing::info!("vendiwm exiting");
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,vendiwm=debug"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

#[derive(Debug)]
enum Backend { Winit, Udev }

// Run nested in a Wayland/X11 session for dev when WAYLAND_DISPLAY or DISPLAY
// is set; otherwise assume we're the session compositor and use udev/DRM.
fn pick_backend() -> Backend {
    if std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var_os("DISPLAY").is_some()
    {
        Backend::Winit
    } else {
        Backend::Udev
    }
}
