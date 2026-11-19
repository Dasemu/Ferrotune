//! Platform-independent tray model: state shown by the tray and the
//! commands it can send back.

use std::sync::Arc;

use tokio::sync::watch;

/// Something the user asked for through the tray icon or its menu.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    /// Left click.
    Toggle,
    Show,
    Hide,
    PlayPause,
    Next,
    Previous,
    /// Scroll on the icon: volume change as a fraction (+ is louder).
    VolumeStep(f32),
    ToggleStartMinimized,
    Quit,
}

/// Volume change per scroll notch (5 %).
pub const VOLUME_STEP: f32 = 0.05;

/// Everything the tray service tells the app.
#[derive(Debug, Clone)]
pub enum Event {
    /// A StatusNotifierWatcher accepted the icon. Carries the handle used to
    /// push state to the tray.
    Ready(Control),
    /// The watcher came back after having gone away.
    Available,
    /// No tray host (there never was one, or the panel went away).
    Unavailable,
    Command(Command),
}

/// What the tray displays. Cheap to compare so republishing an unchanged
/// model costs nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayModel {
    /// Main window currently shown.
    pub visible: bool,
    /// "Artist — Track" of the current track, if any.
    pub track: Option<String>,
    pub playing: bool,
    pub start_minimized: bool,
}

impl Default for TrayModel {
    fn default() -> Self {
        Self {
            visible: true,
            track: None,
            playing: false,
            start_minimized: false,
        }
    }
}

impl TrayModel {
    /// "Artist — Track" while playing, else the app name.
    pub fn tooltip_title(&self) -> String {
        match (&self.track, self.playing) {
            (Some(t), true) => t.clone(),
            _ => crate::config::APP_NAME.to_owned(),
        }
    }

    pub fn toggle_label(&self) -> &'static str {
        if self.visible {
            "Hide Ferrotune"
        } else {
            "Show Ferrotune"
        }
    }

    pub fn play_label(&self) -> &'static str {
        if self.playing { "Pause" } else { "Play" }
    }

    /// Transport entries need something loaded.
    pub fn has_track(&self) -> bool {
        self.track.is_some()
    }
}

/// App-side handle to the running tray: publishes the latest model.
#[derive(Clone)]
pub struct Control {
    tx: Arc<watch::Sender<TrayModel>>,
}

impl std::fmt::Debug for Control {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Control")
    }
}

impl Control {
    pub fn new(tx: Arc<watch::Sender<TrayModel>>) -> Self {
        Self { tx }
    }

    /// Publishes `model`; a no-op when nothing changed.
    pub fn publish(&self, model: TrayModel) {
        self.tx.send_if_modified(|cur| {
            if *cur == model {
                false
            } else {
                *cur = model;
                true
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_shows_the_playing_track_else_the_app_name() {
        let mut m = TrayModel::default();
        assert_eq!(m.tooltip_title(), "Ferrotune");
        m.track = Some("Nova — Midnight".into());
        assert_eq!(m.tooltip_title(), "Ferrotune", "paused/idle shows the name");
        m.playing = true;
        assert_eq!(m.tooltip_title(), "Nova — Midnight");
    }

    #[test]
    fn labels_follow_state() {
        let mut m = TrayModel::default();
        assert_eq!(m.toggle_label(), "Hide Ferrotune");
        assert_eq!(m.play_label(), "Play");
        assert!(!m.has_track());
        m.visible = false;
        m.playing = true;
        m.track = Some("x".into());
        assert_eq!(m.toggle_label(), "Show Ferrotune");
        assert_eq!(m.play_label(), "Pause");
        assert!(m.has_track());
    }

    #[test]
    fn publish_only_wakes_receivers_on_change() {
        let (tx, mut rx) = watch::channel(TrayModel::default());
        let c = Control::new(Arc::new(tx));
        rx.mark_unchanged();
        c.publish(TrayModel::default());
        assert!(!rx.has_changed().unwrap());
        c.publish(TrayModel {
            playing: true,
            ..Default::default()
        });
        assert!(rx.has_changed().unwrap());
        assert!(rx.borrow_and_update().playing);
    }
}
