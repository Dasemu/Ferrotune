//! Audio engine: librespot `Session` + `Player`, or a simulated engine for
//! `--mock` mode and tests. Exposed as a command handle plus an event
//! stream; the app wraps the stream in an iced `Subscription`.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use futures::Stream;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::auth::secrets::Secret;
use crate::config::Bitrate;
use crate::error::{Error, Result};

/// librespot session handle (cheap to clone).
#[derive(Clone)]
pub struct SpotifySession(pub librespot_core::Session);

impl fmt::Debug for SpotifySession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SpotifySession")
    }
}

/// librespot's reusable credentials ("stored credentials" blob), kept only
/// in the keyring.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCredentials {
    pub username: String,
    /// base64 of the opaque auth blob.
    pub auth_data: String,
}

impl fmt::Debug for StoredCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredCredentials")
            .field("auth_data", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl StoredCredentials {
    pub fn to_secret(&self) -> Result<Secret> {
        Ok(Secret::new(serde_json::to_string(self)?))
    }

    pub fn from_secret(s: &Secret) -> Result<Self> {
        serde_json::from_str(s.expose())
            .map_err(|_| Error::auth("stored audio credentials are unreadable"))
    }
}

pub enum SessionCredentials {
    /// Web API OAuth token with the `streaming` scope (first login).
    AccessToken(Secret),
    /// Reusable credentials from a previous login.
    Stored(StoredCredentials),
}

/// Open an audio session. Returns the session and fresh reusable
/// credentials for the keyring. No librespot `Cache` is used, so librespot
/// never writes credentials to disk.
pub async fn connect_session(
    creds: SessionCredentials,
) -> Result<(SpotifySession, StoredCredentials)> {
    use librespot_core::authentication::Credentials;
    let credentials = match creds {
        SessionCredentials::AccessToken(t) => Credentials::with_access_token(t.expose()),
        SessionCredentials::Stored(s) => Credentials {
            username: Some(s.username.clone()),
            auth_type: librespot_protocol::authentication::AuthenticationType::AUTHENTICATION_STORED_SPOTIFY_CREDENTIALS,
            auth_data: B64
                .decode(s.auth_data.as_bytes())
                .map_err(|_| Error::auth("stored audio credentials are corrupt"))?,
        },
    };
    let session = librespot_core::Session::new(librespot_core::SessionConfig::default(), None);
    session
        .connect(credentials, false)
        .await
        .map_err(|e| Error::auth(format!("Audio login failed: {e}")))?;
    let stored = StoredCredentials {
        username: session.username(),
        auth_data: B64.encode(session.auth_data()),
    };
    Ok((SpotifySession(session), stored))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EngineKind {
    Mock,
    Librespot(BitrateKey),
}

/// Hashable mirror of [`Bitrate`] for subscription identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BitrateKey {
    Low,
    Normal,
    High,
}

impl From<Bitrate> for BitrateKey {
    fn from(b: Bitrate) -> Self {
        match b {
            Bitrate::Low => Self::Low,
            Bitrate::Normal => Self::Normal,
            Bitrate::High => Self::High,
        }
    }
}

#[derive(Debug)]
pub enum PlaybackCommand {
    Attach(SpotifySession),
    Load {
        uri: String,
        duration_ms: u32,
        start: bool,
        position_ms: u32,
    },
    Play,
    Pause,
    Stop,
    Seek(u32),
    /// 0.0..=1.0
    SetVolume(f32),
}

#[derive(Clone)]
pub struct PlaybackHandle(mpsc::UnboundedSender<PlaybackCommand>);

impl fmt::Debug for PlaybackHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PlaybackHandle")
    }
}

impl PlaybackHandle {
    pub fn send(&self, cmd: PlaybackCommand) {
        if self.0.send(cmd).is_err() {
            tracing::warn!("playback engine is gone");
        }
    }

    /// Detached handle for tests.
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<PlaybackCommand>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self(tx), rx)
    }
}

#[derive(Debug, Clone)]
pub enum PlaybackEvent {
    Ready(PlaybackHandle),
    Loading { uri: String },
    Playing { uri: String, position_ms: u32 },
    Paused { uri: String, position_ms: u32 },
    Position { uri: String, position_ms: u32 },
    Stopped,
    EndOfTrack { uri: String },
    Unavailable { uri: String },
    Error(String),
}

/// Lazily spawns the engine when first polled and yields its events.
pub fn events(kind: EngineKind) -> impl Stream<Item = PlaybackEvent> + Send + 'static {
    use futures::StreamExt;
    futures::stream::once(async move {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        match kind {
            EngineKind::Mock => {
                tokio::spawn(run_mock(cmd_rx, ev_tx));
            }
            EngineKind::Librespot(b) => {
                tokio::spawn(run_librespot(b, cmd_rx, ev_tx));
            }
        }
        let ready =
            futures::stream::once(async move { PlaybackEvent::Ready(PlaybackHandle(cmd_tx)) });
        let rest =
            futures::stream::unfold(
                ev_rx,
                |mut rx| async move { rx.recv().await.map(|e| (e, rx)) },
            );
        ready.chain(rest)
    })
    .flatten()
}

/// Simulated engine: advances a clock while "playing".
#[derive(Debug, Default)]
pub struct MockEngine {
    pub uri: Option<String>,
    pub duration_ms: u32,
    pub position_ms: u32,
    pub playing: bool,
    pub volume: f32,
}

impl MockEngine {
    /// Apply a command, returning events to emit.
    pub fn command(&mut self, cmd: PlaybackCommand) -> Vec<PlaybackEvent> {
        match cmd {
            PlaybackCommand::Attach(_) => vec![],
            PlaybackCommand::Load {
                uri,
                duration_ms,
                start,
                position_ms,
            } => {
                self.uri = Some(uri.clone());
                self.duration_ms = duration_ms;
                self.position_ms = position_ms;
                self.playing = start;
                let mut v = vec![PlaybackEvent::Loading { uri: uri.clone() }];
                v.push(if start {
                    PlaybackEvent::Playing { uri, position_ms }
                } else {
                    PlaybackEvent::Paused { uri, position_ms }
                });
                v
            }
            PlaybackCommand::Play => self.set_playing(true),
            PlaybackCommand::Pause => self.set_playing(false),
            PlaybackCommand::Stop => {
                self.uri = None;
                self.playing = false;
                vec![PlaybackEvent::Stopped]
            }
            PlaybackCommand::Seek(ms) => {
                self.position_ms = ms.min(self.duration_ms);
                self.status_event().into_iter().collect()
            }
            PlaybackCommand::SetVolume(v) => {
                self.volume = v.clamp(0.0, 1.0);
                vec![]
            }
        }
    }

    fn set_playing(&mut self, playing: bool) -> Vec<PlaybackEvent> {
        if self.uri.is_none() {
            return vec![];
        }
        self.playing = playing;
        self.status_event().into_iter().collect()
    }

    fn status_event(&self) -> Option<PlaybackEvent> {
        let uri = self.uri.clone()?;
        Some(if self.playing {
            PlaybackEvent::Playing {
                uri,
                position_ms: self.position_ms,
            }
        } else {
            PlaybackEvent::Paused {
                uri,
                position_ms: self.position_ms,
            }
        })
    }

    /// Advance the clock by `dt_ms`.
    pub fn tick(&mut self, dt_ms: u32) -> Vec<PlaybackEvent> {
        let Some(uri) = self.uri.clone().filter(|_| self.playing) else {
            return vec![];
        };
        self.position_ms = self.position_ms.saturating_add(dt_ms);
        if self.position_ms >= self.duration_ms {
            self.playing = false;
            self.position_ms = self.duration_ms;
            vec![PlaybackEvent::EndOfTrack { uri }]
        } else {
            vec![PlaybackEvent::Position {
                uri,
                position_ms: self.position_ms,
            }]
        }
    }
}

async fn run_mock(
    mut cmd_rx: mpsc::UnboundedReceiver<PlaybackCommand>,
    ev_tx: mpsc::UnboundedSender<PlaybackEvent>,
) {
    let mut engine = MockEngine::default();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        let events = tokio::select! {
            cmd = cmd_rx.recv() => match cmd {
                Some(c) => engine.command(c),
                None => return,
            },
            _ = tick.tick() => engine.tick(1000),
        };
        for e in events {
            if ev_tx.send(e).is_err() {
                return;
            }
        }
    }
}

fn uri_string(u: &librespot_core::SpotifyUri) -> String {
    u.to_uri().unwrap_or_default()
}

fn map_player_event(e: librespot_playback::player::PlayerEvent) -> Option<PlaybackEvent> {
    use librespot_playback::player::PlayerEvent as P;
    Some(match e {
        P::Loading { track_id, .. } => PlaybackEvent::Loading {
            uri: uri_string(&track_id),
        },
        P::Playing {
            track_id,
            position_ms,
            ..
        } => PlaybackEvent::Playing {
            uri: uri_string(&track_id),
            position_ms,
        },
        P::Paused {
            track_id,
            position_ms,
            ..
        } => PlaybackEvent::Paused {
            uri: uri_string(&track_id),
            position_ms,
        },
        P::PositionChanged {
            track_id,
            position_ms,
            ..
        }
        | P::PositionCorrection {
            track_id,
            position_ms,
            ..
        }
        | P::Seeked {
            track_id,
            position_ms,
            ..
        } => PlaybackEvent::Position {
            uri: uri_string(&track_id),
            position_ms,
        },
        P::EndOfTrack { track_id, .. } => PlaybackEvent::EndOfTrack {
            uri: uri_string(&track_id),
        },
        P::Unavailable { track_id, .. } => PlaybackEvent::Unavailable {
            uri: uri_string(&track_id),
        },
        P::Stopped { .. } => PlaybackEvent::Stopped,
        _ => return None,
    })
}

async fn run_librespot(
    bitrate: BitrateKey,
    mut cmd_rx: mpsc::UnboundedReceiver<PlaybackCommand>,
    ev_tx: mpsc::UnboundedSender<PlaybackEvent>,
) {
    use librespot_playback::audio_backend;
    use librespot_playback::config::{AudioFormat, Bitrate as LBitrate, PlayerConfig};
    use librespot_playback::mixer::softmixer::SoftMixer;
    use librespot_playback::mixer::{Mixer, MixerConfig};
    use librespot_playback::player::Player;

    let mixer: Arc<SoftMixer> = match SoftMixer::open(MixerConfig::default()) {
        Ok(m) => Arc::new(m),
        Err(e) => {
            let _ = ev_tx.send(PlaybackEvent::Error(format!("mixer: {e}")));
            return;
        }
    };
    let mut pending_volume: f32 = 0.7;
    let mut player: Option<Arc<Player>> = None;
    let mut player_events: Option<librespot_playback::player::PlayerEventChannel> = None;

    loop {
        let cmd = tokio::select! {
            cmd = cmd_rx.recv() => match cmd { Some(c) => c, None => return },
            ev = async {
                match player_events.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                match ev {
                    Some(ev) => {
                        if let Some(mapped) = map_player_event(ev)
                            && ev_tx.send(mapped).is_err()
                        {
                            return;
                        }
                    }
                    None => {
                        player_events = None;
                        let _ = ev_tx.send(PlaybackEvent::Error("audio player stopped".into()));
                    }
                }
                continue;
            }
        };
        match cmd {
            PlaybackCommand::Attach(session) => {
                if let Some(p) = &player {
                    p.set_session(session.0);
                    continue;
                }
                let config = PlayerConfig {
                    bitrate: match bitrate {
                        BitrateKey::Low => LBitrate::Bitrate96,
                        BitrateKey::Normal => LBitrate::Bitrate160,
                        BitrateKey::High => LBitrate::Bitrate320,
                    },
                    position_update_interval: Some(Duration::from_secs(1)),
                    ..PlayerConfig::default()
                };
                let Some(backend) = audio_backend::find(None) else {
                    let _ = ev_tx.send(PlaybackEvent::Error("no audio backend".into()));
                    continue;
                };
                mixer.set_volume(volume_to_u16(pending_volume));
                let p = Player::new(config, session.0, mixer.get_soft_volume(), move || {
                    backend(None, AudioFormat::default())
                });
                player_events = Some(p.get_player_event_channel());
                player = Some(p);
            }
            PlaybackCommand::SetVolume(v) => {
                pending_volume = v.clamp(0.0, 1.0);
                mixer.set_volume(volume_to_u16(pending_volume));
            }
            other => {
                let Some(p) = &player else {
                    let _ = ev_tx.send(PlaybackEvent::Error(
                        "audio session is not connected yet".into(),
                    ));
                    continue;
                };
                match other {
                    PlaybackCommand::Load {
                        uri,
                        start,
                        position_ms,
                        ..
                    } => match librespot_core::SpotifyUri::from_uri(&uri) {
                        Ok(u) => p.load(u, start, position_ms),
                        Err(_) => {
                            let _ = ev_tx.send(PlaybackEvent::Unavailable { uri });
                        }
                    },
                    PlaybackCommand::Play => p.play(),
                    PlaybackCommand::Pause => p.pause(),
                    PlaybackCommand::Stop => p.stop(),
                    PlaybackCommand::Seek(ms) => p.seek(ms),
                    PlaybackCommand::Attach(_) | PlaybackCommand::SetVolume(_) => {}
                }
            }
        }
    }
}

/// Map 0.0..=1.0 to librespot's 0..=65535 volume scale.
pub fn volume_to_u16(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * f32::from(u16::MAX)).round() as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(start: bool) -> PlaybackCommand {
        PlaybackCommand::Load {
            uri: "spotify:track:a".into(),
            duration_ms: 2500,
            start,
            position_ms: 0,
        }
    }

    #[test]
    fn mock_engine_plays_to_end() {
        let mut e = MockEngine::default();
        let ev = e.command(load(true));
        assert!(matches!(ev.last(), Some(PlaybackEvent::Playing { .. })));
        assert!(matches!(
            e.tick(1000)[0],
            PlaybackEvent::Position {
                position_ms: 1000,
                ..
            }
        ));
        e.command(PlaybackCommand::Pause);
        assert!(e.tick(1000).is_empty(), "paused clock must not advance");
        e.command(PlaybackCommand::Play);
        e.tick(1000);
        assert!(matches!(e.tick(1000)[0], PlaybackEvent::EndOfTrack { .. }));
    }

    #[test]
    fn mock_engine_seek_and_stop() {
        let mut e = MockEngine::default();
        assert!(
            e.command(PlaybackCommand::Play).is_empty(),
            "nothing loaded"
        );
        e.command(load(false));
        let ev = e.command(PlaybackCommand::Seek(99_999));
        assert!(matches!(
            ev[0],
            PlaybackEvent::Paused {
                position_ms: 2500,
                ..
            }
        ));
        assert!(matches!(
            e.command(PlaybackCommand::Stop)[0],
            PlaybackEvent::Stopped
        ));
    }

    #[test]
    fn volume_scale() {
        assert_eq!(volume_to_u16(0.0), 0);
        assert_eq!(volume_to_u16(1.0), 65535);
        assert_eq!(volume_to_u16(2.0), 65535);
        assert_eq!(volume_to_u16(0.5), 32768);
    }

    #[test]
    fn stored_credentials_roundtrip_and_redaction() {
        let c = StoredCredentials {
            username: "user".into(),
            auth_data: "c2VjcmV0".into(),
        };
        assert_eq!(
            StoredCredentials::from_secret(&c.to_secret().unwrap()).unwrap(),
            c
        );
        assert!(!format!("{c:?}").contains("c2VjcmV0"));
    }
}
