// Backends abstract how vendiWM gets input events and presents frames.
//
// `winit` runs vendiWM as a nested Wayland client inside another compositor
// (Hyprland, sway, GNOME). Used during development for iteration.
//
// `udev` runs vendiWM as the session compositor — talks directly to DRM/KMS
// for output, libinput for input, libseat for hotplug. Used in production.

#[cfg(feature = "winit")]
pub mod winit;

#[cfg(feature = "udev")]
pub mod udev;
