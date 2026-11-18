//! MPRIS (org.mpris.MediaPlayer2 + .Player) on the D-Bus session bus, so
//! KDE media keys and the Plasma media widget control Ferrotune.
//!
//! The platform-neutral part ([`MediaSnapshot`], [`MediaCommand`]) is always
//! compiled; the D-Bus server only on Linux.

use std::fmt;

use tokio::sync::mpsc;

pub const BUS_SUFFIX: &str = "ferrotune";
pub const TRACK_PATH_PREFIX: &str = "/dev/ferro/ferrotune/track/";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MediaStatus {
    #[default]
    Stopped,
    Playing,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MediaLoop {
    #[default]
    None,
    Playlist,
    Track,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MediaTrack {
    /// Base62 Spotify id (used to build the MPRIS track object path).
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: String,
    pub length_ms: u32,
    pub art_url: Option<String>,
}

/// Everything MPRIS clients can observe. Produced by the app on each change.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MediaSnapshot {
    pub status: MediaStatus,
    pub track: Option<MediaTrack>,
    pub position_ms: u32,
    /// 0.0..=1.0
    pub volume: f64,
    pub shuffle: bool,
    pub loop_status: MediaLoop,
    pub can_next: bool,
    pub can_previous: bool,
}

/// Requests coming from MPRIS clients.
#[derive(Debug, Clone, PartialEq)]
pub enum MediaCommand {
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    /// Relative seek in microseconds.
    Seek(i64),
    /// Absolute position in microseconds for the given track id.
    SetPosition {
        track_id: String,
        position_us: i64,
    },
    SetVolume(f64),
    SetShuffle(bool),
    SetLoop(MediaLoop),
    Raise,
    Quit,
}

#[derive(Clone)]
pub struct MediaHandle(mpsc::UnboundedSender<MediaSnapshot>);

impl fmt::Debug for MediaHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MediaHandle")
    }
}

impl MediaHandle {
    pub fn update(&self, snapshot: MediaSnapshot) {
        let _ = self.0.send(snapshot);
    }

    pub fn detached() -> (Self, mpsc::UnboundedReceiver<MediaSnapshot>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self(tx), rx)
    }
}

#[derive(Debug, Clone)]
pub enum MediaEvent {
    Ready(MediaHandle),
    Command(MediaCommand),
    Unavailable(String),
}

/// Track id from an MPRIS object path we produced.
pub fn track_id_from_path(path: &str) -> Option<&str> {
    path.strip_prefix(TRACK_PATH_PREFIX)
}

/// Object path for a track id; D-Bus paths only allow `[A-Za-z0-9_]`.
pub fn track_path(id: &str) -> String {
    let clean: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!(
        "{TRACK_PATH_PREFIX}{}",
        if clean.is_empty() {
            "none".into()
        } else {
            clean
        }
    )
}

#[cfg(target_os = "linux")]
pub use linux::{events, serve};

#[cfg(not(target_os = "linux"))]
pub fn events() -> impl futures::Stream<Item = MediaEvent> + Send + 'static {
    futures::stream::empty()
}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::{Arc, Mutex};

    use futures::Stream;
    use mpris_server::zbus::{self, fdo};
    use mpris_server::{
        LoopStatus, Metadata, PlaybackRate, PlaybackStatus, PlayerInterface, Property,
        RootInterface, Server, Signal, Time, TrackId, Volume,
    };
    use tokio::sync::mpsc;

    use super::*;
    use crate::config::{APP_ID, APP_NAME};

    pub fn map_status(s: MediaStatus) -> PlaybackStatus {
        match s {
            MediaStatus::Stopped => PlaybackStatus::Stopped,
            MediaStatus::Playing => PlaybackStatus::Playing,
            MediaStatus::Paused => PlaybackStatus::Paused,
        }
    }

    pub fn map_loop(l: MediaLoop) -> LoopStatus {
        match l {
            MediaLoop::None => LoopStatus::None,
            MediaLoop::Playlist => LoopStatus::Playlist,
            MediaLoop::Track => LoopStatus::Track,
        }
    }

    pub fn unmap_loop(l: LoopStatus) -> MediaLoop {
        match l {
            LoopStatus::None => MediaLoop::None,
            LoopStatus::Playlist => MediaLoop::Playlist,
            LoopStatus::Track => MediaLoop::Track,
        }
    }

    pub fn map_metadata(t: Option<&MediaTrack>) -> Metadata {
        let Some(t) = t else {
            return Metadata::builder().trackid(TrackId::NO_TRACK).build();
        };
        let mut b = Metadata::builder()
            .title(t.title.clone())
            .artist(t.artists.clone())
            .album(t.album.clone())
            .length(Time::from_millis(i64::from(t.length_ms)));
        if let Ok(id) = TrackId::try_from(track_path(&t.id)) {
            b = b.trackid(id);
        }
        if let Some(art) = &t.art_url {
            b = b.art_url(art.clone());
        }
        b.build()
    }

    /// The `PropertiesChanged` payload between two snapshots.
    pub fn diff(old: &MediaSnapshot, new: &MediaSnapshot) -> Vec<Property> {
        let mut v = Vec::new();
        if old.status != new.status {
            v.push(Property::PlaybackStatus(map_status(new.status)));
        }
        if old.track != new.track {
            v.push(Property::Metadata(map_metadata(new.track.as_ref())));
        }
        if (old.volume - new.volume).abs() > f64::EPSILON {
            v.push(Property::Volume(new.volume));
        }
        if old.shuffle != new.shuffle {
            v.push(Property::Shuffle(new.shuffle));
        }
        if old.loop_status != new.loop_status {
            v.push(Property::LoopStatus(map_loop(new.loop_status)));
        }
        if old.can_next != new.can_next {
            v.push(Property::CanGoNext(new.can_next));
        }
        if old.can_previous != new.can_previous {
            v.push(Property::CanGoPrevious(new.can_previous));
        }
        v
    }

    /// True when the position moved in a way clients can't extrapolate
    /// (seek), which requires a `Seeked` signal.
    pub fn is_seek(old: &MediaSnapshot, new: &MediaSnapshot, elapsed_ms: u64) -> bool {
        if old.track != new.track {
            return false;
        }
        let expected = if old.status == MediaStatus::Playing {
            u64::from(old.position_ms) + elapsed_ms
        } else {
            u64::from(old.position_ms)
        };
        u64::from(new.position_ms).abs_diff(expected) > 2500
    }

    pub struct FerroMpris {
        state: Arc<Mutex<MediaSnapshot>>,
        cmd: mpsc::UnboundedSender<MediaCommand>,
    }

    impl FerroMpris {
        fn snap(&self) -> MediaSnapshot {
            self.state.lock().map(|s| s.clone()).unwrap_or_default()
        }

        fn send(&self, c: MediaCommand) -> fdo::Result<()> {
            self.cmd
                .send(c)
                .map_err(|_| fdo::Error::Failed("player is shutting down".into()))
        }
    }

    impl RootInterface for FerroMpris {
        async fn raise(&self) -> fdo::Result<()> {
            self.send(MediaCommand::Raise)
        }
        async fn quit(&self) -> fdo::Result<()> {
            self.send(MediaCommand::Quit)
        }
        async fn can_quit(&self) -> fdo::Result<bool> {
            Ok(true)
        }
        async fn fullscreen(&self) -> fdo::Result<bool> {
            Ok(false)
        }
        async fn set_fullscreen(&self, _fullscreen: bool) -> zbus::Result<()> {
            Ok(())
        }
        async fn can_set_fullscreen(&self) -> fdo::Result<bool> {
            Ok(false)
        }
        async fn can_raise(&self) -> fdo::Result<bool> {
            Ok(true)
        }
        async fn has_track_list(&self) -> fdo::Result<bool> {
            Ok(false)
        }
        async fn identity(&self) -> fdo::Result<String> {
            Ok(APP_NAME.into())
        }
        async fn desktop_entry(&self) -> fdo::Result<String> {
            Ok(APP_ID.into())
        }
        async fn supported_uri_schemes(&self) -> fdo::Result<Vec<String>> {
            Ok(vec![])
        }
        async fn supported_mime_types(&self) -> fdo::Result<Vec<String>> {
            Ok(vec![])
        }
    }

    impl PlayerInterface for FerroMpris {
        async fn next(&self) -> fdo::Result<()> {
            self.send(MediaCommand::Next)
        }
        async fn previous(&self) -> fdo::Result<()> {
            self.send(MediaCommand::Previous)
        }
        async fn pause(&self) -> fdo::Result<()> {
            self.send(MediaCommand::Pause)
        }
        async fn play_pause(&self) -> fdo::Result<()> {
            self.send(MediaCommand::PlayPause)
        }
        async fn stop(&self) -> fdo::Result<()> {
            self.send(MediaCommand::Stop)
        }
        async fn play(&self) -> fdo::Result<()> {
            self.send(MediaCommand::Play)
        }
        async fn seek(&self, offset: Time) -> fdo::Result<()> {
            self.send(MediaCommand::Seek(offset.as_micros()))
        }
        async fn set_position(&self, track_id: TrackId, position: Time) -> fdo::Result<()> {
            let id = track_id_from_path(track_id.as_str())
                .unwrap_or_default()
                .to_owned();
            self.send(MediaCommand::SetPosition {
                track_id: id,
                position_us: position.as_micros(),
            })
        }
        async fn open_uri(&self, _uri: String) -> fdo::Result<()> {
            Err(fdo::Error::NotSupported("OpenUri is not supported".into()))
        }
        async fn playback_status(&self) -> fdo::Result<PlaybackStatus> {
            Ok(map_status(self.snap().status))
        }
        async fn loop_status(&self) -> fdo::Result<LoopStatus> {
            Ok(map_loop(self.snap().loop_status))
        }
        async fn set_loop_status(&self, loop_status: LoopStatus) -> zbus::Result<()> {
            self.cmd
                .send(MediaCommand::SetLoop(unmap_loop(loop_status)))
                .map_err(|_| zbus::Error::Failure("player is shutting down".into()))
        }
        async fn rate(&self) -> fdo::Result<PlaybackRate> {
            Ok(1.0)
        }
        async fn set_rate(&self, _rate: PlaybackRate) -> zbus::Result<()> {
            Ok(())
        }
        async fn shuffle(&self) -> fdo::Result<bool> {
            Ok(self.snap().shuffle)
        }
        async fn set_shuffle(&self, shuffle: bool) -> zbus::Result<()> {
            self.cmd
                .send(MediaCommand::SetShuffle(shuffle))
                .map_err(|_| zbus::Error::Failure("player is shutting down".into()))
        }
        async fn metadata(&self) -> fdo::Result<Metadata> {
            Ok(map_metadata(self.snap().track.as_ref()))
        }
        async fn volume(&self) -> fdo::Result<Volume> {
            Ok(self.snap().volume)
        }
        async fn set_volume(&self, volume: Volume) -> zbus::Result<()> {
            self.cmd
                .send(MediaCommand::SetVolume(volume.clamp(0.0, 1.0)))
                .map_err(|_| zbus::Error::Failure("player is shutting down".into()))
        }
        async fn position(&self) -> fdo::Result<Time> {
            Ok(Time::from_millis(i64::from(self.snap().position_ms)))
        }
        async fn minimum_rate(&self) -> fdo::Result<PlaybackRate> {
            Ok(1.0)
        }
        async fn maximum_rate(&self) -> fdo::Result<PlaybackRate> {
            Ok(1.0)
        }
        async fn can_go_next(&self) -> fdo::Result<bool> {
            Ok(self.snap().can_next)
        }
        async fn can_go_previous(&self) -> fdo::Result<bool> {
            Ok(self.snap().can_previous)
        }
        async fn can_play(&self) -> fdo::Result<bool> {
            Ok(self.snap().track.is_some())
        }
        async fn can_pause(&self) -> fdo::Result<bool> {
            Ok(self.snap().track.is_some())
        }
        async fn can_seek(&self) -> fdo::Result<bool> {
            Ok(self.snap().track.is_some())
        }
        async fn can_control(&self) -> fdo::Result<bool> {
            Ok(true)
        }
    }

    /// Register on the session bus and serve until `updates` closes.
    /// Commands from clients are sent to `commands`.
    pub async fn serve(
        bus_suffix: &str,
        mut updates: mpsc::UnboundedReceiver<MediaSnapshot>,
        commands: mpsc::UnboundedSender<MediaCommand>,
    ) -> zbus::Result<()> {
        let state = Arc::new(Mutex::new(MediaSnapshot::default()));
        let server = Server::new(
            bus_suffix,
            FerroMpris {
                state: state.clone(),
                cmd: commands,
            },
        )
        .await?;
        tracing::info!(bus = %server.bus_name(), "MPRIS registered");
        let mut last_update = std::time::Instant::now();
        while let Some(new) = updates.recv().await {
            let old = {
                let mut s = state
                    .lock()
                    .map_err(|_| zbus::Error::Failure("poisoned".into()))?;
                std::mem::replace(&mut *s, new.clone())
            };
            let elapsed = last_update.elapsed().as_millis() as u64;
            last_update = std::time::Instant::now();
            let changes = diff(&old, &new);
            if !changes.is_empty()
                && let Err(e) = server.properties_changed(changes).await
            {
                tracing::warn!(error = %e, "MPRIS PropertiesChanged failed");
            }
            if is_seek(&old, &new, elapsed) {
                let _ = server
                    .emit(Signal::Seeked {
                        position: Time::from_millis(i64::from(new.position_ms)),
                    })
                    .await;
            }
        }
        Ok(())
    }

    /// Event stream for the app: `Ready(handle)` then client commands.
    pub fn events() -> impl Stream<Item = MediaEvent> + Send + 'static {
        use futures::StreamExt;
        futures::stream::once(async move {
            let (snap_tx, snap_rx) = mpsc::unbounded_channel();
            let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
            let (err_tx, err_rx) = tokio::sync::oneshot::channel::<String>();
            tokio::spawn(async move {
                if let Err(e) = serve(BUS_SUFFIX, snap_rx, cmd_tx).await {
                    tracing::warn!(error = %e, "MPRIS unavailable");
                    let _ = err_tx.send(e.to_string());
                }
            });
            let ready =
                futures::stream::once(async move { MediaEvent::Ready(MediaHandle(snap_tx)) });
            let cmds = futures::stream::unfold(cmd_rx, |mut rx| async move {
                rx.recv().await.map(|c| (MediaEvent::Command(c), rx))
            });
            let err = futures::stream::once(err_rx)
                .filter_map(|r| async move { r.ok().map(MediaEvent::Unavailable) });
            ready.chain(futures::stream::select(cmds, err))
        })
        .flatten()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn track() -> MediaTrack {
            MediaTrack {
                id: "4uLU6hMCjMI75M1A2tKUQC".into(),
                title: "Song".into(),
                artists: vec!["A".into(), "B".into()],
                album: "Album".into(),
                length_ms: 200_000,
                art_url: Some("https://i.scdn.co/image/x".into()),
            }
        }

        #[test]
        fn metadata_mapping() {
            let m = map_metadata(Some(&track()));
            assert_eq!(m.title(), Some("Song"));
            assert_eq!(m.artist(), Some(vec!["A".to_string(), "B".to_string()]));
            assert_eq!(m.album(), Some("Album"));
            assert_eq!(m.length(), Some(Time::from_millis(200_000)));
            assert_eq!(
                m.trackid().unwrap().as_str(),
                "/dev/ferro/ferrotune/track/4uLU6hMCjMI75M1A2tKUQC"
            );
            assert_eq!(m.art_url().as_deref(), Some("https://i.scdn.co/image/x"));
            let empty = map_metadata(None);
            assert_eq!(empty.trackid(), Some(TrackId::NO_TRACK));
        }

        #[test]
        fn status_and_loop_mapping() {
            assert_eq!(map_status(MediaStatus::Playing), PlaybackStatus::Playing);
            assert_eq!(map_status(MediaStatus::Paused), PlaybackStatus::Paused);
            assert_eq!(map_status(MediaStatus::Stopped), PlaybackStatus::Stopped);
            for l in [MediaLoop::None, MediaLoop::Playlist, MediaLoop::Track] {
                assert_eq!(unmap_loop(map_loop(l)), l);
            }
        }

        #[test]
        fn diff_reports_only_changes() {
            let a = MediaSnapshot::default();
            let mut b = a.clone();
            assert!(diff(&a, &b).is_empty());
            b.status = MediaStatus::Playing;
            b.track = Some(track());
            b.shuffle = true;
            let d = diff(&a, &b);
            assert_eq!(d.len(), 3);
            assert!(d.contains(&Property::PlaybackStatus(PlaybackStatus::Playing)));
            assert!(d.contains(&Property::Shuffle(true)));
        }

        #[test]
        fn seek_detection() {
            let mut a = MediaSnapshot {
                status: MediaStatus::Playing,
                track: Some(track()),
                position_ms: 10_000,
                ..Default::default()
            };
            let mut b = a.clone();
            b.position_ms = 11_000;
            assert!(!is_seek(&a, &b, 1000), "normal progress");
            b.position_ms = 60_000;
            assert!(is_seek(&a, &b, 1000), "jump forward");
            a.status = MediaStatus::Paused;
            b.position_ms = 10_000;
            assert!(!is_seek(&a, &b, 5000), "paused keeps position");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_paths() {
        let p = track_path("abc123");
        assert_eq!(track_id_from_path(&p), Some("abc123"));
        assert_eq!(track_path("a:b-c"), "/dev/ferro/ferrotune/track/a_b_c");
        assert_eq!(track_path(""), "/dev/ferro/ferrotune/track/none");
    }
}
