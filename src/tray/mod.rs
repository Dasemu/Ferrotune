//! System tray / background mode.
//!
//! * `model` (pure, all platforms): what the tray shows (`TrayModel`) and what
//!   it can ask for (`Command`). Unit-tested without D-Bus.
//! * `sni` (Linux): the StatusNotifierItem service, built on `ksni`.
//! * `stub` (other platforms): no tray; the app behaves as before
//!   (closing the window quits). Not implemented on Windows / macOS.
//! * `autostart`: the XDG autostart entry written by the settings toggle.
//! * `instance`: single-instance lock; a second launch asks the first one to
//!   show its window.
//!
//! Like `net`, nothing here imports iced widgets.

pub mod autostart;
pub mod instance;
pub mod model;

#[cfg(target_os = "linux")]
mod icon;
#[cfg(target_os = "linux")]
mod sni;
#[cfg(target_os = "linux")]
pub use sni::events;

#[cfg(not(target_os = "linux"))]
mod stub;
#[cfg(not(target_os = "linux"))]
pub use stub::events;

pub use model::{Command, Control, Event, TrayModel};
