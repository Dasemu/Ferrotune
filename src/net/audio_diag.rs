//! Why did librespot report a track as `Unavailable`?
//!
//! librespot 0.8 emits `PlayerEvent::Unavailable` for every load failure
//! (restricted track, no supported file format, metadata/CDN errors, audio
//! key refused, decoder error) and keeps the reason in its log only. A small
//! `tracing` layer ([`DiagLayer`]) watches librespot's own error/warning
//! records for the player and the audio-key manager and remembers the last
//! ones; the engine reads them when `Unavailable` arrives and classifies the
//! failure with the pure [`classify`].
//!
//! Only categories and Spotify's numeric codes are kept: no track names,
//! ids or user data leave librespot's message text.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// The stage at which a load failed, as far as librespot's log tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnavailableReason {
    /// The access point refused the audio key with a permanent code
    /// (`error audio key 0 1`). Spotify is phasing these requests out for
    /// some accounts (librespot issue #1649); no client-side fix exists.
    AudioKeyDenied { code: u16 },
    /// The access point refused the key with a temporary code (`0 2`).
    AudioKeyTransient { code: u16 },
    /// Region / catalogue restriction or embargo ("Track is unavailable").
    Restricted,
    /// No file in a format librespot can decode.
    NoSupportedFormat,
    /// Metadata, storage-resolve or CDN request failed.
    Fetch,
    /// The file downloaded but could not be decoded.
    Decode,
    /// Nothing in the log explains it.
    Unknown,
}

impl UnavailableReason {
    pub fn is_key_denied(self) -> bool {
        matches!(self, Self::AudioKeyDenied { .. })
    }

    /// Short, user-facing explanation.
    pub fn describe(self) -> &'static str {
        match self {
            Self::AudioKeyDenied { .. } => {
                "Spotify is not allowing playback on this device for your account (audio key denied)."
            }
            Self::AudioKeyTransient { .. } => "Spotify did not hand out the audio key in time.",
            Self::Restricted => "This song is not available in your country.",
            Self::NoSupportedFormat => "This song is not available in a supported format.",
            Self::Fetch => "The song could not be downloaded.",
            Self::Decode => "The song could not be decoded.",
            Self::Unknown => "This song is not available.",
        }
    }
}

/// Parse librespot's `error audio key {:x} {:x}` into one code.
pub fn parse_key_error(message: &str) -> Option<u16> {
    let rest = message.trim().strip_prefix("error audio key ")?;
    let mut it = rest.split_whitespace();
    let hi = u16::from_str_radix(it.next()?, 16).ok()?;
    let lo = u16::from_str_radix(it.next()?, 16).ok()?;
    Some(hi.checked_mul(256)?.checked_add(lo)?)
}

/// Category of one of librespot's player error/warning messages.
pub fn player_message_kind(message: &str) -> Option<UnavailableReason> {
    let m = message.trim();
    Some(if m.starts_with("Track is unavailable") {
        UnavailableReason::Restricted
    } else if m.contains("is not available in any supported format") {
        UnavailableReason::NoSupportedFormat
    } else if m.starts_with("Unable to load audio item")
        || m.starts_with("Unable to load encrypted file")
        || m.starts_with("Track should be available, but no alternatives found")
        || m.contains("could not be converted to a base62 ID")
    {
        UnavailableReason::Fetch
    } else if m.starts_with("Unable to read audio file")
        || m.contains("error opening subfile")
        || m.contains("error seeking to starting position")
    {
        UnavailableReason::Decode
    } else {
        return None;
    })
}

/// Decide the reason from what was logged during the load.
///
/// A key refusal wins over the decoder error it causes: librespot carries
/// on without decryption and then fails to decode the encrypted bytes.
pub fn classify(key_code: Option<u16>, player: Option<UnavailableReason>) -> UnavailableReason {
    match key_code {
        Some(2) => UnavailableReason::AudioKeyTransient { code: 2 },
        Some(code) => UnavailableReason::AudioKeyDenied { code },
        None => player.unwrap_or(UnavailableReason::Unknown),
    }
}

/// How long a logged failure stays attributable to the next `Unavailable`.
pub const WINDOW: Duration = Duration::from_secs(30);

#[derive(Debug, Default)]
struct Recent {
    key: Option<(Instant, u16)>,
    player: Option<(Instant, UnavailableReason)>,
}

/// What librespot logged recently.
#[derive(Debug, Default)]
pub struct Recorder {
    inner: Mutex<Recent>,
}

impl Recorder {
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(Recent {
                key: None,
                player: None,
            }),
        }
    }

    /// Feed one log record (`target`, rendered message).
    pub fn record(&self, target: &str, message: &str, now: Instant) {
        let Ok(mut r) = self.inner.lock() else { return };
        if target.starts_with("librespot_core::audio_key") {
            if let Some(code) = parse_key_error(message) {
                r.key = Some((now, code));
            }
        } else if target.starts_with("librespot_playback")
            && let Some(kind) = player_message_kind(message)
        {
            // Keep the most specific stage: a later generic message must not
            // hide the restriction that caused it.
            let keep = matches!(
                r.player,
                Some((at, UnavailableReason::Restricted | UnavailableReason::NoSupportedFormat))
                    if now.saturating_duration_since(at) < Duration::from_secs(2)
            );
            if !keep {
                r.player = Some((now, kind));
            }
        }
    }

    /// Classify and forget what was logged in the last [`WINDOW`].
    pub fn take(&self, now: Instant) -> UnavailableReason {
        let Ok(mut r) = self.inner.lock() else {
            return UnavailableReason::Unknown;
        };
        let fresh = |at: Instant| now.saturating_duration_since(at) <= WINDOW;
        let key = r.key.take().filter(|(at, _)| fresh(*at)).map(|(_, c)| c);
        let player = r.player.take().filter(|(at, _)| fresh(*at)).map(|(_, k)| k);
        classify(key, player)
    }
}

/// Process-wide recorder fed by [`DiagLayer`].
pub static RECORDER: Recorder = Recorder::new();

/// `tracing` layer feeding [`RECORDER`] from librespot's log records
/// (bridged from the `log` crate by `tracing-log`).
#[derive(Debug, Default, Clone, Copy)]
pub struct DiagLayer;

#[derive(Default)]
struct Fields {
    message: String,
    log_target: Option<String>,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.message = value.to_owned(),
            "log.target" => self.log_target = Some(value.to_owned()),
            _ => {}
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        }
    }
}

impl<S: Subscriber> Layer<S> for DiagLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        if *meta.level() > tracing::Level::WARN {
            return;
        }
        let mut f = Fields::default();
        event.record(&mut f);
        let target = f.log_target.as_deref().unwrap_or(meta.target());
        if target.starts_with("librespot") {
            RECORDER.record(target, &f.message, Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_errors_parse() {
        assert_eq!(parse_key_error("error audio key 0 1"), Some(1));
        assert_eq!(parse_key_error("error audio key 0 2"), Some(2));
        assert_eq!(parse_key_error("error audio key 1 0"), Some(256));
        assert_eq!(parse_key_error("error audio key zz 1"), None);
        assert_eq!(parse_key_error("Audio key response timeout"), None);
    }

    #[test]
    fn player_messages_map_to_stages() {
        use UnavailableReason as R;
        assert_eq!(
            player_message_kind("Track is unavailable: NotWhitelisted"),
            Some(R::Restricted)
        );
        assert_eq!(
            player_message_kind("<x> is not available in any supported format"),
            Some(R::NoSupportedFormat)
        );
        assert_eq!(
            player_message_kind("Unable to load audio item: Error { .. }"),
            Some(R::Fetch)
        );
        assert_eq!(
            player_message_kind(
                "Unable to read audio file: Symphonia Decoder Error: end of stream"
            ),
            Some(R::Decode)
        );
        assert_eq!(player_message_kind("Loading <x>"), None);
    }

    #[test]
    fn key_refusal_beats_the_decoder_error_it_causes() {
        use UnavailableReason as R;
        assert_eq!(
            classify(Some(1), Some(R::Decode)),
            R::AudioKeyDenied { code: 1 }
        );
        assert_eq!(classify(Some(2), None), R::AudioKeyTransient { code: 2 });
        assert_eq!(classify(None, Some(R::Restricted)), R::Restricted);
        assert_eq!(classify(None, None), R::Unknown);
    }

    #[test]
    fn recorder_matches_the_real_log_sequence() {
        // The sequence from a real v0.4.0 run (audio key denied).
        let r = Recorder::new();
        let t = Instant::now();
        r.record("librespot_core::audio_key", "error audio key 0 1", t);
        r.record(
            "librespot_playback::player",
            "Unable to load key, continuing without decryption: Service unavailable { audio key error }",
            t,
        );
        r.record(
            "librespot_playback::player",
            "Unable to read audio file: Symphonia Decoder Error: end of stream",
            t,
        );
        assert_eq!(r.take(t), UnavailableReason::AudioKeyDenied { code: 1 });
        // Taken: the next failure starts from scratch.
        assert_eq!(r.take(t), UnavailableReason::Unknown);
    }

    #[test]
    fn recorder_forgets_stale_records_and_keeps_specific_stage() {
        let r = Recorder::new();
        let t = Instant::now();
        r.record("librespot_core::audio_key", "error audio key 0 1", t);
        assert_eq!(
            r.take(t + WINDOW + Duration::from_secs(1)),
            UnavailableReason::Unknown
        );
        r.record(
            "librespot_playback::player",
            "Track is unavailable: Embargo",
            t,
        );
        r.record(
            "librespot_playback::player",
            "Unable to load audio item: x",
            t + Duration::from_millis(10),
        );
        assert_eq!(r.take(t), UnavailableReason::Restricted);
        // Other crates' messages are ignored.
        r.record("ferrotune::app", "Track is unavailable", t);
        assert_eq!(r.take(t), UnavailableReason::Unknown);
    }

    #[test]
    fn layer_sees_bridged_and_native_events() {
        use tracing_subscriber::prelude::*;
        let sub = tracing_subscriber::registry().with(DiagLayer);
        tracing::subscriber::with_default(sub, || {
            tracing::error!(target: "librespot_core::audio_key", "error audio key 0 1");
        });
        assert_eq!(
            RECORDER.take(Instant::now()),
            UnavailableReason::AudioKeyDenied { code: 1 }
        );
    }
}
