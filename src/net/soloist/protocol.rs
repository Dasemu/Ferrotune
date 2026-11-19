//! Spotify Soloist's local WebSocket API (docs: "WebSocket API reference").
//!
//! All frames are JSON text. The client sends `{"type": "command",
//! "command": <name>, ...fields}`; the server sends events with a `type`
//! field. Everything here is pure and unit-tested: parsing is lenient
//! (unknown events and fields are ignored, missing fields default), so a
//! newer Soloist build cannot crash or confuse the app.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::net::playback::RemoteTrack;

/// Largest frame we accept from Soloist (queue snapshots are the biggest).
pub const MAX_FRAME: usize = 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Identity {
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Cover {
    pub url: String,
    pub size: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct VisualIdentity {
    pub cover: Vec<Cover>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Related {
    pub entity: Option<Box<Entity>>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct PlaybackInfo {
    pub duration_ms: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Decorations {
    pub identity: Option<Identity>,
    pub visual_identity: Option<VisualIdentity>,
    pub parent: Option<Related>,
    pub creators: Vec<Related>,
    pub playback: Option<PlaybackInfo>,
}

/// The common entity envelope (track, episode, album, playlist...).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Entity {
    pub uri: String,
    pub entity_type: String,
    pub decorations: Decorations,
}

impl Entity {
    pub fn name(&self) -> String {
        self.decorations
            .identity
            .as_ref()
            .map(|i| i.name.clone())
            .unwrap_or_default()
    }

    pub fn duration_ms(&self) -> u32 {
        let d = self
            .decorations
            .playback
            .as_ref()
            .map_or(0.0, |p| p.duration_ms);
        clamp_ms(d)
    }

    /// Best cover for the player bar: `large`, then `default`, then any.
    /// Only `https://` URLs are used (the image loader fetches them).
    pub fn cover_url(&self) -> Option<String> {
        let covers = &self.decorations.visual_identity.as_ref()?.cover;
        let ok = |c: &&Cover| c.url.starts_with("https://");
        ["large", "default", "xlarge", "small"]
            .iter()
            .find_map(|s| covers.iter().filter(ok).find(|c| c.size == *s))
            .or_else(|| covers.iter().find(ok))
            .map(|c| c.url.clone())
    }

    /// Converted for the player bar and MPRIS.
    pub fn to_remote_track(&self) -> RemoteTrack {
        let d = &self.decorations;
        RemoteTrack {
            uri: self.uri.clone(),
            name: self.name(),
            artists: d
                .creators
                .iter()
                .filter_map(|c| c.entity.as_ref().map(|e| e.name()))
                .filter(|n| !n.is_empty())
                .collect(),
            album: d
                .parent
                .as_ref()
                .and_then(|p| p.entity.as_ref())
                .map(|e| e.name())
                .unwrap_or_default(),
            cover_url: self.cover_url(),
            duration_ms: self.duration_ms(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Idle,
    Playing,
    Paused,
    Buffering,
    #[serde(other)]
    Unknown,
}

/// Position anchor: `position_ms` at server time `timestamp_ms`,
/// advancing at `speed` (0.0 while paused).
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Position {
    pub position_ms: f64,
    pub timestamp_ms: f64,
    pub speed: f64,
}

impl Position {
    /// Position at wall-clock `now_ms` (Unix epoch ms), never negative.
    pub fn at(&self, now_ms: u64) -> u32 {
        let mut p = self.position_ms;
        let speed = if self.speed.is_finite() {
            self.speed
        } else {
            0.0
        };
        if speed > 0.0 && self.timestamp_ms > 0.0 {
            let elapsed = (now_ms as f64 - self.timestamp_ms).max(0.0);
            p += elapsed * speed;
        }
        clamp_ms(p)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RepeatMode {
    #[default]
    Off,
    Context,
    Track,
}

impl<'de> Deserialize<'de> for RepeatMode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "context" => Self::Context,
            "track" => Self::Track,
            _ => Self::Off,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Options {
    pub shuffle: bool,
    pub repeat: RepeatMode,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct PlaybackSnapshot {
    pub status: Status,
    pub item: Option<Entity>,
    pub context: Option<Entity>,
    pub position: Position,
    pub volume: f64,
    pub is_active: bool,
    pub options: Options,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct QueueEntry {
    pub uid: String,
    pub source: String,
    pub item: Entity,
}

/// Server-to-client events.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    AuthState {
        #[serde(default)]
        logged_in: bool,
        #[serde(default)]
        is_active: bool,
        #[serde(default)]
        device_name: String,
    },
    PlaybackState(Box<PlaybackSnapshot>),
    TrackChanged {
        #[serde(default)]
        item: Entity,
    },
    PlaybackChanged {
        #[serde(default)]
        status: Status,
    },
    VolumeChanged {
        #[serde(default)]
        volume: f64,
    },
    DeviceChanged {
        #[serde(default)]
        is_active: bool,
        #[serde(default)]
        device_name: String,
    },
    ContextChanged {
        #[serde(default)]
        context: Entity,
    },
    OptionsChanged {
        #[serde(default)]
        options: Options,
    },
    PositionSync {
        #[serde(default)]
        position: Position,
    },
    QueueChanged {
        #[serde(default)]
        previous: Vec<QueueEntry>,
        #[serde(default)]
        upcoming: Vec<QueueEntry>,
    },
    CommandResult {
        #[serde(default)]
        command: String,
    },
    Error {
        #[serde(default)]
        message: String,
    },
    #[serde(other)]
    Unknown,
}

/// Parse one text frame. Oversized or malformed frames give `None`.
pub fn parse_event(text: &str) -> Option<Event> {
    if text.len() > MAX_FRAME {
        return None;
    }
    serde_json::from_str(text).ok()
}

/// Client-to-server commands, exactly as documented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    GetAuthState,
    GetState,
    /// `0` = all entries.
    GetQueue(u32),
    /// Resume, or start a playable Spotify URI.
    Play(Option<String>),
    Pause,
    SkipNext,
    SkipPrev,
    Seek(u32),
    /// 0..=100
    SetVolume(u8),
    SetShuffle(bool),
    SetRepeatContext(bool),
    SetRepeatTrack(bool),
    /// Track URIs only.
    AddToQueue(String),
    Activate,
    Deactivate,
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Self::GetAuthState => "get_auth_state",
            Self::GetState => "get_state",
            Self::GetQueue(_) => "get_queue",
            Self::Play(_) => "play",
            Self::Pause => "pause",
            Self::SkipNext => "skip_next",
            Self::SkipPrev => "skip_prev",
            Self::Seek(_) => "seek",
            Self::SetVolume(_) => "set_volume",
            Self::SetShuffle(_) => "set_shuffle",
            Self::SetRepeatContext(_) => "set_repeat_context",
            Self::SetRepeatTrack(_) => "set_repeat_track",
            Self::AddToQueue(_) => "add_to_queue",
            Self::Activate => "activate",
            Self::Deactivate => "deactivate",
        }
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({"type": "command", "command": self.name()});
        match self {
            Self::GetQueue(limit) => v["limit"] = json!(limit),
            Self::Play(Some(uri)) | Self::AddToQueue(uri) => v["uri"] = json!(uri),
            Self::Seek(ms) => v["position_ms"] = json!(ms),
            Self::SetVolume(vol) => v["volume"] = json!((*vol).min(100)),
            Self::SetShuffle(on) | Self::SetRepeatContext(on) | Self::SetRepeatTrack(on) => {
                v["enabled"] = json!(on);
            }
            _ => {}
        }
        v
    }
}

/// The documented two-command sequence for a repeat mode.
pub fn repeat_commands(mode: RepeatMode) -> [Command; 2] {
    match mode {
        RepeatMode::Off => [
            Command::SetRepeatTrack(false),
            Command::SetRepeatContext(false),
        ],
        RepeatMode::Context => [
            Command::SetRepeatTrack(false),
            Command::SetRepeatContext(true),
        ],
        RepeatMode::Track => [
            Command::SetRepeatContext(false),
            Command::SetRepeatTrack(true),
        ],
    }
}

/// 0.0..=1.0 -> Soloist's 0..=100.
pub fn volume_percent(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 100.0).round() as u8
}

/// Soloist's 0..=100 -> 0.0..=1.0 (out-of-range values are clamped).
pub fn volume_unit(v: f64) -> f32 {
    if v.is_finite() {
        (v.clamp(0.0, 100.0) / 100.0) as f32
    } else {
        0.0
    }
}

fn clamp_ms(v: f64) -> u32 {
    if v.is_finite() && v > 0.0 {
        v.min(f64::from(u32::MAX)) as u32
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `playback_state` example from the WebSocket API reference.
    pub const DOC_PLAYBACK_STATE: &str = r#"{
      "type": "playback_state",
      "status": "playing",
      "item": {
        "uri": "spotify:track:2JRo0gjbX4GrCqBYdRohoo",
        "entity_type": "track",
        "decorations": {
          "identity": {"name": "My Song"},
          "playback": {"duration_ms": 210000, "content_ratings": []}
        }
      },
      "context": {
        "uri": "spotify:playlist:37i9dQZF1DXcBWIGoYBM5M",
        "entity_type": "playlist",
        "decorations": {"identity": {"name": "Today's Top Hits"}}
      },
      "position": {"position_ms": 45000, "timestamp_ms": 1747654321000, "speed": 1.0},
      "volume": 65,
      "is_active": true,
      "options": {"shuffle": false, "repeat": "off", "playback_speed": 1.0, "modes": {}},
      "available_actions": {"pause": {}, "seek": {}, "seek_forward": {"step_ms": 15000}}
    }"#;

    /// The entity example from the reference (with parent and creators).
    const DOC_ENTITY: &str = r#"{
      "uri": "spotify:track:2JRo0gjbX4GrCqBYdRohoo",
      "entity_type": "track",
      "decorations": {
        "identity": {"name": "My Song"},
        "visual_identity": {"cover": [
          {"url": "https://i.scdn.co/image/small", "size": "small"},
          {"url": "https://i.scdn.co/image/large", "size": "large"}
        ]},
        "parent": {"entity": {"uri": "spotify:album:4aawyAB9vmqN3uQ7FjRGTy",
          "entity_type": "album", "decorations": {"identity": {"name": "Album Name"}}}},
        "creators": [{"entity": {"uri": "spotify:artist:x", "entity_type": "artist",
          "decorations": {"identity": {"name": "Artist Name"}}}}],
        "playback": {"duration_ms": 210000, "content_ratings": []}
      }
    }"#;

    #[test]
    fn parses_the_documented_playback_state() {
        let Some(Event::PlaybackState(s)) = parse_event(DOC_PLAYBACK_STATE) else {
            panic!("not a playback_state");
        };
        assert_eq!(s.status, Status::Playing);
        assert!(s.is_active);
        assert_eq!(s.volume, 65.0);
        assert_eq!(s.options.repeat, RepeatMode::Off);
        let item = s.item.unwrap();
        assert_eq!(item.name(), "My Song");
        assert_eq!(item.duration_ms(), 210_000);
        assert_eq!(s.context.unwrap().name(), "Today's Top Hits");
        // 2 s after the anchor at speed 1.0.
        assert_eq!(s.position.at(1_747_654_323_000), 47_000);
    }

    #[test]
    fn entity_converts_for_the_player_bar() {
        let e: Entity = serde_json::from_str(DOC_ENTITY).unwrap();
        let t = e.to_remote_track();
        assert_eq!(t.name, "My Song");
        assert_eq!(t.artists, vec!["Artist Name".to_owned()]);
        assert_eq!(t.album, "Album Name");
        assert_eq!(
            t.cover_url.as_deref(),
            Some("https://i.scdn.co/image/large")
        );
        assert_eq!(t.duration_ms, 210_000);
    }

    #[test]
    fn parses_granular_events() {
        let ev = |s: &str| parse_event(s).unwrap();
        assert_eq!(
            ev(r#"{"type":"auth_state","logged_in":true,"is_active":false,"device_name":"K"}"#),
            Event::AuthState {
                logged_in: true,
                is_active: false,
                device_name: "K".into()
            }
        );
        assert_eq!(
            ev(r#"{"type":"playback_changed","status":"paused"}"#),
            Event::PlaybackChanged {
                status: Status::Paused
            }
        );
        assert_eq!(
            ev(r#"{"type":"volume_changed","volume":42}"#),
            Event::VolumeChanged { volume: 42.0 }
        );
        assert_eq!(
            ev(r#"{"type":"device_changed","is_active":true,"device_name":"Kitchen speaker"}"#),
            Event::DeviceChanged {
                is_active: true,
                device_name: "Kitchen speaker".into()
            }
        );
        let Event::OptionsChanged { options } =
            ev(r#"{"type":"options_changed","options":{"shuffle":true,"repeat":"track"}}"#)
        else {
            panic!()
        };
        assert!(options.shuffle);
        assert_eq!(options.repeat, RepeatMode::Track);
        assert_eq!(
            ev(r#"{"type":"command_result","command":"pause"}"#),
            Event::CommandResult {
                command: "pause".into()
            }
        );
        assert_eq!(
            ev(r#"{"type":"error","message":"command requires authentication"}"#),
            Event::Error {
                message: "command requires authentication".into()
            }
        );
        let Event::QueueChanged { previous, upcoming } = ev(
            r#"{"type":"queue_changed","previous":[{"uid":"a","source":"context","item":{"uri":"spotify:track:previous","entity_type":"track","decorations":{}}}],"upcoming":[{"uid":"b","source":"queue","item":{"uri":"spotify:track:upcoming","entity_type":"track","decorations":{"identity":{"name":"Next Song"}}}}]}"#,
        ) else {
            panic!()
        };
        assert_eq!(previous.len(), 1);
        assert_eq!(upcoming[0].item.name(), "Next Song");
        assert_eq!(upcoming[0].source, "queue");
    }

    #[test]
    fn lenient_parsing() {
        // Unknown events, unknown statuses and missing fields never fail.
        assert_eq!(
            parse_event(r#"{"type":"something_new","x":1}"#),
            Some(Event::Unknown)
        );
        assert_eq!(
            parse_event(r#"{"type":"playback_changed","status":"rewinding"}"#),
            Some(Event::PlaybackChanged {
                status: Status::Unknown
            })
        );
        assert!(matches!(
            parse_event(r#"{"type":"playback_state"}"#),
            Some(Event::PlaybackState(_))
        ));
        assert_eq!(parse_event("not json"), None);
        assert_eq!(parse_event(r#"{"no":"type"}"#), None);
        let huge = format!(
            r#"{{"type":"error","message":"{}"}}"#,
            "x".repeat(MAX_FRAME)
        );
        assert_eq!(parse_event(&huge), None);
        // Hostile numbers are clamped, not panicking.
        let p = Position {
            position_ms: -5.0,
            timestamp_ms: f64::NAN,
            speed: f64::INFINITY,
        };
        assert_eq!(p.at(10), 0);
        let e: Entity = serde_json::from_str(
            r#"{"decorations":{"playback":{"duration_ms":1e300},"visual_identity":{"cover":[{"url":"http://plain","size":"large"}]}}}"#,
        )
        .unwrap();
        assert_eq!(e.duration_ms(), u32::MAX);
        assert_eq!(e.cover_url(), None, "only https covers are loaded");
    }

    #[test]
    fn paused_position_does_not_advance() {
        let p = Position {
            position_ms: 1000.0,
            timestamp_ms: 1000.0,
            speed: 0.0,
        };
        assert_eq!(p.at(999_999), 1000);
    }

    #[test]
    fn command_json_matches_the_reference() {
        let j = |c: Command| c.to_json().to_string();
        assert_eq!(
            j(Command::Play(None)),
            r#"{"command":"play","type":"command"}"#
        );
        let cases = [
            (Command::Pause, r#"{"type":"command","command":"pause"}"#),
            (
                Command::Play(Some("spotify:playlist:37i9dQZF1DXcBWIGoYBM5M".into())),
                r#"{"type":"command","command":"play","uri":"spotify:playlist:37i9dQZF1DXcBWIGoYBM5M"}"#,
            ),
            (
                Command::Seek(30000),
                r#"{"type":"command","command":"seek","position_ms":30000}"#,
            ),
            (
                Command::SetVolume(50),
                r#"{"type":"command","command":"set_volume","volume":50}"#,
            ),
            (
                Command::SetShuffle(true),
                r#"{"type":"command","command":"set_shuffle","enabled":true}"#,
            ),
            (
                Command::AddToQueue("spotify:track:6rqhFgbbKwnb9MLmUQDhG6".into()),
                r#"{"type":"command","command":"add_to_queue","uri":"spotify:track:6rqhFgbbKwnb9MLmUQDhG6"}"#,
            ),
            (
                Command::GetQueue(10),
                r#"{"type":"command","command":"get_queue","limit":10}"#,
            ),
            (
                Command::GetState,
                r#"{"type":"command","command":"get_state"}"#,
            ),
            (
                Command::SkipNext,
                r#"{"type":"command","command":"skip_next"}"#,
            ),
            (
                Command::SkipPrev,
                r#"{"type":"command","command":"skip_prev"}"#,
            ),
            (
                Command::Activate,
                r#"{"type":"command","command":"activate"}"#,
            ),
            (
                Command::Deactivate,
                r#"{"type":"command","command":"deactivate"}"#,
            ),
            (
                Command::GetAuthState,
                r#"{"type":"command","command":"get_auth_state"}"#,
            ),
        ];
        for (cmd, want) in cases {
            let want: Value = serde_json::from_str(want).unwrap();
            assert_eq!(cmd.to_json(), want, "{cmd:?}");
        }
        assert_eq!(Command::SetVolume(250).to_json()["volume"], 100);
    }

    #[test]
    fn repeat_sequences_follow_the_reference() {
        use Command::*;
        assert_eq!(
            repeat_commands(RepeatMode::Off),
            [SetRepeatTrack(false), SetRepeatContext(false)]
        );
        assert_eq!(
            repeat_commands(RepeatMode::Context),
            [SetRepeatTrack(false), SetRepeatContext(true)]
        );
        assert_eq!(
            repeat_commands(RepeatMode::Track),
            [SetRepeatContext(false), SetRepeatTrack(true)]
        );
    }

    #[test]
    fn volume_scales() {
        assert_eq!(volume_percent(0.5), 50);
        assert_eq!(volume_percent(2.0), 100);
        assert_eq!(volume_unit(42.0), 0.42);
        assert_eq!(volume_unit(-1.0), 0.0);
        assert_eq!(volume_unit(f64::NAN), 0.0);
    }
}
