//! StatusNotifierItem tray (KDE Plasma and other modern panels) via `ksni`.

use std::sync::Arc;
use std::time::Duration;

use futures::Stream;
use ksni::menu::{CheckmarkItem, StandardItem};
use ksni::{Category, MenuItem, Orientation, Status, ToolTip, TrayMethods};
use tokio::sync::{mpsc, watch};

use super::model::{Command, Control, Event, TrayModel, VOLUME_STEP};

/// How long registering with the StatusNotifierWatcher may take before the
/// tray counts as unavailable.
const REGISTER_TIMEOUT: Duration = Duration::from_secs(5);

struct SniTray {
    model: TrayModel,
    icons: Arc<Vec<ksni::Icon>>,
    tx: mpsc::UnboundedSender<Event>,
}

impl SniTray {
    fn send(&self, c: Command) {
        let _ = self.tx.send(Event::Command(c));
    }
}

impl ksni::Tray for SniTray {
    fn id(&self) -> String {
        crate::config::APP_ID.to_owned()
    }

    fn title(&self) -> String {
        crate::config::APP_NAME.to_owned()
    }

    fn category(&self) -> Category {
        Category::ApplicationStatus
    }

    /// There is no unread or attention state in a music player.
    fn status(&self) -> Status {
        Status::Active
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icons.as_ref().clone()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            icon_name: String::new(),
            icon_pixmap: self.icons.as_ref().clone(),
            title: self.model.tooltip_title(),
            description: String::new(),
        }
    }

    /// Left click.
    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Command::Toggle);
    }

    /// Wheel over the icon: volume, like the official client.
    fn scroll(&mut self, delta: i32, orientation: Orientation) {
        if orientation != Orientation::Vertical || delta == 0 {
            return;
        }
        let step = if delta > 0 { VOLUME_STEP } else { -VOLUME_STEP };
        self.send(Command::VolumeStep(step));
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let m = &self.model;
        vec![
            StandardItem {
                label: m.toggle_label().into(),
                activate: Box::new(|t: &mut Self| t.send(Command::Toggle)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: m.play_label().into(),
                enabled: m.has_track(),
                activate: Box::new(|t: &mut Self| t.send(Command::PlayPause)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Next".into(),
                enabled: m.has_track(),
                activate: Box::new(|t: &mut Self| t.send(Command::Next)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Previous".into(),
                enabled: m.has_track(),
                activate: Box::new(|t: &mut Self| t.send(Command::Previous)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            CheckmarkItem {
                label: "Start minimized to tray".into(),
                checked: m.start_minimized,
                activate: Box::new(|t: &mut Self| t.send(Command::ToggleStartMinimized)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit Ferrotune".into(),
                activate: Box::new(|t: &mut Self| t.send(Command::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }

    fn watcher_online(&self) {
        let _ = self.tx.send(Event::Available);
    }

    /// The panel went away: tell the app so a hidden window comes back
    /// instead of leaving an unreachable process. The service keeps running
    /// and re-registers when a watcher returns.
    fn watcher_offline(&self, _reason: ksni::OfflineReason) -> bool {
        let _ = self.tx.send(Event::Unavailable);
        true
    }
}

/// Starts the tray on the current tokio runtime and streams its events.
///
/// The first event is `Ready` (registered with a StatusNotifierWatcher) or
/// `Unavailable` (no D-Bus session, no watcher, or a timeout). Dropping the
/// stream shuts the tray down.
pub fn events() -> impl Stream<Item = Event> + Send + 'static {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(run(tx));
    futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|e| (e, rx)) })
}

async fn run(tx: mpsc::UnboundedSender<Event>) {
    let Some(icons) = tokio::task::spawn_blocking(super::icon::render)
        .await
        .ok()
        .flatten()
    else {
        tracing::warn!("tray icon could not be rendered");
        let _ = tx.send(Event::Unavailable);
        return;
    };
    let (model_tx, mut model_rx) = watch::channel(TrayModel::default());
    let model_tx = Arc::new(model_tx);
    let tray = SniTray {
        model: TrayModel::default(),
        icons: Arc::new(icons),
        tx: tx.clone(),
    };
    let handle = match tokio::time::timeout(REGISTER_TIMEOUT, tray.spawn()).await {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => {
            tracing::info!(error = %e, "no system tray available");
            let _ = tx.send(Event::Unavailable);
            return;
        }
        Err(_) => {
            tracing::info!("system tray registration timed out");
            let _ = tx.send(Event::Unavailable);
            return;
        }
    };
    tracing::info!("system tray registered");
    let _ = tx.send(Event::Ready(Control::new(model_tx.clone())));
    loop {
        tokio::select! {
            changed = model_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let m = model_rx.borrow_and_update().clone();
                if handle.update(move |t| t.model = m).await.is_none() {
                    let _ = tx.send(Event::Unavailable);
                    break;
                }
            }
            // The subscription was dropped.
            _ = tx.closed() => {
                handle.shutdown().await;
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksni::Tray;

    fn tray(model: TrayModel) -> (SniTray, mpsc::UnboundedReceiver<Event>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let icons = super::super::icon::render().unwrap();
        (
            SniTray {
                model,
                icons: Arc::new(icons),
                tx,
            },
            rx,
        )
    }

    fn drain(rx: &mut mpsc::UnboundedReceiver<Event>) -> Vec<Command> {
        let mut out = Vec::new();
        while let Ok(Event::Command(c)) = rx.try_recv() {
            out.push(c);
        }
        out
    }

    fn labels(items: &[MenuItem<SniTray>]) -> Vec<String> {
        items
            .iter()
            .filter_map(|i| match i {
                MenuItem::Standard(s) => Some(s.label.clone()),
                MenuItem::Checkmark(c) => Some(c.label.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn menu_has_the_expected_entries() {
        let (t, _rx) = tray(TrayModel::default());
        assert_eq!(
            labels(&t.menu()),
            [
                "Hide Ferrotune",
                "Play",
                "Next",
                "Previous",
                "Start minimized to tray",
                "Quit Ferrotune"
            ]
        );
        let (t, _rx) = tray(TrayModel {
            visible: false,
            playing: true,
            track: Some("A — B".into()),
            ..Default::default()
        });
        let l = labels(&t.menu());
        assert_eq!(l[0], "Show Ferrotune");
        assert_eq!(l[1], "Pause");
    }

    #[test]
    fn transport_is_disabled_without_a_track() {
        let (t, _rx) = tray(TrayModel::default());
        let menu = t.menu();
        let MenuItem::Standard(play) = &menu[2] else {
            panic!("play");
        };
        assert!(!play.enabled);
        let (t, _rx) = tray(TrayModel {
            track: Some("A — B".into()),
            ..Default::default()
        });
        let menu = t.menu();
        let MenuItem::Standard(play) = &menu[2] else {
            panic!("play");
        };
        assert!(play.enabled);
    }

    #[test]
    fn menu_items_send_their_commands() {
        let (mut t, mut rx) = tray(TrayModel {
            track: Some("A — B".into()),
            ..Default::default()
        });
        let menu = t.menu();
        for item in &menu {
            match item {
                MenuItem::Standard(s) => (s.activate)(&mut t),
                MenuItem::Checkmark(c) => (c.activate)(&mut t),
                _ => {}
            }
        }
        assert_eq!(
            drain(&mut rx),
            [
                Command::Toggle,
                Command::PlayPause,
                Command::Next,
                Command::Previous,
                Command::ToggleStartMinimized,
                Command::Quit,
            ]
        );
    }

    #[test]
    fn left_click_toggles_and_scroll_changes_volume() {
        let (mut t, mut rx) = tray(TrayModel::default());
        t.activate(0, 0);
        t.scroll(1, Orientation::Vertical);
        t.scroll(-1, Orientation::Vertical);
        t.scroll(1, Orientation::Horizontal);
        t.scroll(0, Orientation::Vertical);
        assert_eq!(
            drain(&mut rx),
            [
                Command::Toggle,
                Command::VolumeStep(VOLUME_STEP),
                Command::VolumeStep(-VOLUME_STEP)
            ]
        );
    }

    #[test]
    fn status_stays_active_and_tooltip_names_the_track() {
        let (mut t, _rx) = tray(TrayModel::default());
        assert_eq!(t.status(), Status::Active);
        assert_eq!(t.tool_tip().title, "Ferrotune");
        t.model.track = Some("Nova — Midnight".into());
        t.model.playing = true;
        assert_eq!(t.status(), Status::Active);
        assert_eq!(t.tool_tip().title, "Nova — Midnight");
        assert_eq!(t.icon_pixmap().len(), 3);
    }

    #[test]
    fn watcher_loss_is_reported_but_keeps_the_service() {
        let (t, mut rx) = tray(TrayModel::default());
        assert!(t.watcher_offline(ksni::OfflineReason::No));
        assert!(matches!(rx.try_recv(), Ok(Event::Unavailable)));
        t.watcher_online();
        assert!(matches!(rx.try_recv(), Ok(Event::Available)));
    }
}
