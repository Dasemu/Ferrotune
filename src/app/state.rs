//! Pure state types and reducers (unit-tested without iced).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use rand::SeedableRng;
use rand::rngs::StdRng;
use rand::seq::{IndexedRandom, SliceRandom};

use crate::net::model::Track;

// ---------------------------------------------------------------- navigation

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Route {
    Home,
    Search,
    Library,
    Liked,
    Playlist(String),
    Album(String),
    Artist(String),
}

impl Route {
    pub fn key(&self) -> String {
        match self {
            Self::Home => "home".into(),
            Self::Search => "search".into(),
            Self::Library => "library".into(),
            Self::Liked => "liked".into(),
            Self::Playlist(id) => format!("playlist:{id}"),
            Self::Album(id) => format!("album:{id}"),
            Self::Artist(id) => format!("artist:{id}"),
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key.split_once(':') {
            None => match key {
                "home" => Self::Home,
                "search" => Self::Search,
                "library" => Self::Library,
                "liked" => Self::Liked,
                _ => return None,
            },
            Some(("playlist", id)) => Self::Playlist(id.into()),
            Some(("album", id)) => Self::Album(id.into()),
            Some(("artist", id)) => Self::Artist(id.into()),
            Some(_) => return None,
        })
    }
}

/// Back/forward history like a browser.
#[derive(Debug, Clone)]
pub struct Nav {
    stack: Vec<Route>,
    index: usize,
}

pub const NAV_MAX: usize = 50;

impl Nav {
    pub fn new(start: Route) -> Self {
        Self {
            stack: vec![start],
            index: 0,
        }
    }

    pub fn current(&self) -> &Route {
        &self.stack[self.index]
    }

    /// Navigate to `route`, dropping forward history. No-op for the
    /// current route.
    pub fn push(&mut self, route: Route) -> bool {
        if *self.current() == route {
            return false;
        }
        self.stack.truncate(self.index + 1);
        self.stack.push(route);
        if self.stack.len() > NAV_MAX {
            self.stack.remove(0);
        }
        self.index = self.stack.len() - 1;
        true
    }

    pub fn back(&mut self) -> bool {
        if self.can_back() {
            self.index -= 1;
            true
        } else {
            false
        }
    }

    pub fn forward(&mut self) -> bool {
        if self.can_forward() {
            self.index += 1;
            true
        } else {
            false
        }
    }

    pub fn can_back(&self) -> bool {
        self.index > 0
    }

    pub fn can_forward(&self) -> bool {
        self.index + 1 < self.stack.len()
    }
}

// --------------------------------------------------------------------- queue

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    Off,
    Context,
    Track,
}

impl Repeat {
    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::Context,
            Self::Context => Self::Track,
            Self::Track => Self::Off,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Context => "context",
            Self::Track => "track",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "context" => Self::Context,
            "track" => Self::Track,
            _ => Self::Off,
        }
    }
}

/// Where the queue's tracks came from ("Playing from playlist X").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayContext {
    pub name: String,
    pub route: Option<Route>,
}

/// What `previous()` asks the player to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Previous {
    Restart,
    Play(Track),
}

/// Restart the current track instead of going back after this position.
pub const RESTART_THRESHOLD_MS: u32 = 3000;

/// Playback queue: a context (album/playlist) in play order, plus the
/// user's explicit "Next in queue" items which always play first.
#[derive(Debug)]
pub struct Queue {
    pub context: Option<PlayContext>,
    tracks: Vec<Track>,
    /// Play order: indices into `tracks` (identity or shuffled).
    order: Vec<usize>,
    /// Position in `order` of the last context track played.
    pos: usize,
    pub user_queue: VecDeque<Track>,
    pub current: Option<Track>,
    pub shuffle: bool,
    pub repeat: Repeat,
    rng: StdRng,
}

impl Default for Queue {
    fn default() -> Self {
        Self::with_seed(rand::random())
    }
}

impl Queue {
    pub fn with_seed(seed: u64) -> Self {
        Self {
            context: None,
            tracks: Vec::new(),
            order: Vec::new(),
            pos: 0,
            user_queue: VecDeque::new(),
            current: None,
            shuffle: false,
            repeat: Repeat::Off,
            rng: StdRng::seed_from_u64(seed),
        }
    }

    pub fn context_tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// Start playing `tracks` at `start` (or a random track when `None`
    /// and shuffle is on). Returns the track to load.
    pub fn play_context(
        &mut self,
        context: PlayContext,
        tracks: Vec<Track>,
        start: Option<usize>,
    ) -> Option<Track> {
        if tracks.is_empty() {
            return None;
        }
        let start = match start {
            Some(i) => i.min(tracks.len() - 1),
            None if self.shuffle => {
                let playable: Vec<usize> =
                    (0..tracks.len()).filter(|&i| tracks[i].playable).collect();
                *playable.choose(&mut self.rng).unwrap_or(&0)
            }
            None => 0,
        };
        self.context = Some(context);
        self.tracks = tracks;
        self.rebuild_order(start);
        let mut first = self.tracks[self.order[self.pos]].clone();
        if !first.playable {
            match self.step_context(true) {
                Some(t) => first = t,
                None => return None,
            }
        }
        self.current = Some(first.clone());
        Some(first)
    }

    /// Recompute `order` so that context index `current` is at `pos`.
    fn rebuild_order(&mut self, current: usize) {
        let n = self.tracks.len();
        if self.shuffle {
            let mut rest: Vec<usize> = (0..n).filter(|&i| i != current).collect();
            rest.shuffle(&mut self.rng);
            self.order = std::iter::once(current).chain(rest).collect();
            self.pos = 0;
        } else {
            self.order = (0..n).collect();
            self.pos = current;
        }
    }

    /// Advance within the context, skipping unplayable tracks.
    /// `wrap` allows wrapping to the start (repeat context).
    fn step_context(&mut self, wrap: bool) -> Option<Track> {
        let n = self.order.len();
        for _ in 0..n {
            if self.pos + 1 < n {
                self.pos += 1;
            } else if wrap {
                if self.shuffle {
                    // A new shuffle round, like the official client.
                    let mut o = self.order.clone();
                    o.shuffle(&mut self.rng);
                    self.order = o;
                }
                self.pos = 0;
            } else {
                return None;
            }
            let t = &self.tracks[self.order[self.pos]];
            if t.playable {
                return Some(t.clone());
            }
        }
        None
    }

    /// Next track. `auto` is true at end-of-track (repeat-one applies).
    pub fn next(&mut self, auto: bool) -> Option<Track> {
        if auto && self.repeat == Repeat::Track {
            return self.current.clone();
        }
        let next = match self.user_queue.pop_front() {
            Some(t) => Some(t),
            None => {
                let wrap = self.repeat != Repeat::Off;
                self.step_context(wrap)
            }
        };
        match next {
            Some(t) => {
                self.current = Some(t.clone());
                Some(t)
            }
            None => None,
        }
    }

    pub fn previous(&mut self, position_ms: u32) -> Previous {
        if position_ms > RESTART_THRESHOLD_MS || self.order.is_empty() {
            return Previous::Restart;
        }
        let mut p = self.pos;
        for _ in 0..self.order.len() {
            if p > 0 {
                p -= 1;
            } else if self.repeat != Repeat::Off {
                p = self.order.len() - 1;
            } else {
                return Previous::Restart;
            }
            let t = &self.tracks[self.order[p]];
            if t.playable {
                self.pos = p;
                self.current = Some(t.clone());
                return Previous::Play(t.clone());
            }
        }
        Previous::Restart
    }

    pub fn set_shuffle(&mut self, on: bool) {
        if self.shuffle == on {
            return;
        }
        self.shuffle = on;
        if self.tracks.is_empty() {
            return;
        }
        let current = self.order.get(self.pos).copied().unwrap_or(0);
        self.rebuild_order(current);
    }

    pub fn cycle_repeat(&mut self) -> Repeat {
        self.repeat = self.repeat.cycle();
        self.repeat
    }

    pub fn enqueue(&mut self, track: Track) {
        self.user_queue.push_back(track);
    }

    /// Upcoming context tracks after the current one (for the queue panel).
    pub fn upcoming(&self, limit: usize) -> Vec<&Track> {
        self.order
            .iter()
            .skip(self.pos + 1)
            .map(|&i| &self.tracks[i])
            .filter(|t| t.playable)
            .take(limit)
            .collect()
    }

    /// Jump to the k-th upcoming context track (queue panel click).
    pub fn jump_upcoming(&mut self, k: usize) -> Option<Track> {
        let mut seen = 0;
        for p in self.pos + 1..self.order.len() {
            let t = &self.tracks[self.order[p]];
            if !t.playable {
                continue;
            }
            if seen == k {
                self.pos = p;
                self.current = Some(t.clone());
                return Some(t.clone());
            }
            seen += 1;
        }
        None
    }

    /// Play the k-th "Next in queue" item now (removing items before it).
    pub fn jump_user_queue(&mut self, k: usize) -> Option<Track> {
        if k >= self.user_queue.len() {
            return None;
        }
        self.user_queue.drain(..k);
        let t = self.user_queue.pop_front()?;
        self.current = Some(t.clone());
        Some(t)
    }

    pub fn has_next(&self) -> bool {
        !self.user_queue.is_empty()
            || self.repeat != Repeat::Off && !self.order.is_empty()
            || self
                .order
                .iter()
                .skip(self.pos + 1)
                .any(|&i| self.tracks[i].playable)
    }

    pub fn clear(&mut self) {
        self.context = None;
        self.tracks.clear();
        self.order.clear();
        self.pos = 0;
        self.user_queue.clear();
        self.current = None;
    }
}

// ------------------------------------------------------------------ playback

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayStatus {
    #[default]
    Stopped,
    Loading,
    Playing,
    Paused,
}

/// Engine-reported playback position, interpolated between reports.
#[derive(Debug, Clone)]
pub struct Playback {
    pub status: PlayStatus,
    pub position_ms: u32,
    pub anchor: Instant,
    pub duration_ms: u32,
    /// 0.0..=1.0
    pub volume: f32,
    pub muted: bool,
    /// While dragging the seek bar.
    pub seek_preview: Option<u32>,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            status: PlayStatus::Stopped,
            position_ms: 0,
            anchor: Instant::now(),
            duration_ms: 0,
            volume: 0.7,
            muted: false,
            seek_preview: None,
        }
    }
}

impl Playback {
    pub fn report(&mut self, status: PlayStatus, position_ms: u32, now: Instant) {
        self.status = status;
        self.position_ms = position_ms;
        self.anchor = now;
    }

    pub fn position(&self, now: Instant) -> u32 {
        if let Some(p) = self.seek_preview {
            return p;
        }
        let p = if self.status == PlayStatus::Playing {
            let dt = now.saturating_duration_since(self.anchor).as_millis() as u32;
            self.position_ms.saturating_add(dt)
        } else {
            self.position_ms
        };
        if self.duration_ms > 0 {
            p.min(self.duration_ms)
        } else {
            p
        }
    }

    pub fn effective_volume(&self) -> f32 {
        if self.muted { 0.0 } else { self.volume }
    }

    pub fn is_playing(&self) -> bool {
        matches!(self.status, PlayStatus::Playing | PlayStatus::Loading)
    }
}

// ---------------------------------------------------------------- connection

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Connection {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Reconnecting {
        attempt: u32,
    },
}

/// Exponential backoff with jitter: 1 s, 2 s, 4 s … capped at 60 s.
/// `jitter` in `[0, 1)` spreads the delay by ±20 %.
pub fn backoff_delay(attempt: u32, jitter: f64) -> Duration {
    let base = 1000u64.saturating_mul(1u64 << attempt.min(6)).min(60_000) as f64;
    let factor = 0.8 + 0.4 * jitter.clamp(0.0, 1.0);
    Duration::from_millis((base * factor).min(60_000.0) as u64)
}

// -------------------------------------------------------------------- filters

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LibraryFilter {
    #[default]
    All,
    Playlists,
    Albums,
    Artists,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchFilter {
    #[default]
    All,
    Songs,
    Artists,
    Albums,
    Playlists,
}

/// "3:07" / "1:02:03"
pub fn format_duration(ms: u32) -> String {
    let s = ms / 1000;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// "1 hr 3 min" / "42 min 10 sec" for collection headers.
pub fn format_total(ms: u64) -> String {
    let s = ms / 1000;
    let (h, m, sec) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h} hr {m} min")
    } else {
        format!("{m} min {sec} sec")
    }
}

/// "1,234,567"
pub fn format_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str) -> Track {
        Track {
            id: id.into(),
            uri: format!("spotify:track:{id}"),
            name: id.into(),
            artists: vec![],
            album: None,
            duration_ms: 1000,
            explicit: false,
            playable: true,
            added_at: None,
        }
    }

    fn tracks(n: usize) -> Vec<Track> {
        (0..n).map(|i| t(&format!("t{i}"))).collect()
    }

    fn ctx() -> PlayContext {
        PlayContext {
            name: "ctx".into(),
            route: None,
        }
    }

    fn ids(q: &Queue) -> Vec<String> {
        q.upcoming(100).iter().map(|t| t.id.clone()).collect()
    }

    #[test]
    fn linear_play_and_stop_at_end() {
        let mut q = Queue::with_seed(1);
        assert_eq!(q.play_context(ctx(), tracks(3), Some(1)).unwrap().id, "t1");
        assert_eq!(ids(&q), vec!["t2"]);
        assert_eq!(q.next(true).unwrap().id, "t2");
        assert!(q.next(true).is_none(), "repeat off stops at the end");
        assert!(!q.has_next());
    }

    #[test]
    fn repeat_context_wraps_and_repeat_track_repeats() {
        let mut q = Queue::with_seed(1);
        q.play_context(ctx(), tracks(2), Some(0));
        q.cycle_repeat();
        assert_eq!(q.repeat, Repeat::Context);
        assert_eq!(q.next(true).unwrap().id, "t1");
        assert_eq!(q.next(true).unwrap().id, "t0", "wraps");
        q.cycle_repeat();
        assert_eq!(q.repeat, Repeat::Track);
        assert_eq!(q.next(true).unwrap().id, "t0", "auto-advance repeats");
        assert_eq!(q.next(false).unwrap().id, "t1", "manual next still skips");
        q.cycle_repeat();
        assert_eq!(q.repeat, Repeat::Off);
    }

    #[test]
    fn user_queue_plays_first_then_context_resumes() {
        let mut q = Queue::with_seed(1);
        q.play_context(ctx(), tracks(3), Some(0));
        q.enqueue(t("x"));
        q.enqueue(t("y"));
        assert_eq!(q.next(false).unwrap().id, "x");
        assert_eq!(q.next(false).unwrap().id, "y");
        assert_eq!(q.next(false).unwrap().id, "t1");
        q.enqueue(t("a"));
        q.enqueue(t("b"));
        assert_eq!(q.jump_user_queue(1).unwrap().id, "b");
        assert!(q.user_queue.is_empty());
    }

    #[test]
    fn shuffle_keeps_current_and_covers_all() {
        let mut q = Queue::with_seed(42);
        q.play_context(ctx(), tracks(10), Some(4));
        q.set_shuffle(true);
        assert_eq!(q.current.as_ref().unwrap().id, "t4");
        let mut up = ids(&q);
        assert_eq!(up.len(), 9);
        assert!(!up.contains(&"t4".to_string()));
        up.sort();
        let mut expect: Vec<String> = (0..10)
            .filter(|&i| i != 4)
            .map(|i| format!("t{i}"))
            .collect();
        expect.sort();
        assert_eq!(up, expect);
        // Turning shuffle off resumes in order after the current track.
        q.next(false);
        let cur = q.current.clone().unwrap();
        q.set_shuffle(false);
        let n: usize = cur.id[1..].parse().unwrap();
        assert_eq!(
            ids(&q).first().cloned(),
            (n + 1 < 10).then(|| format!("t{}", n + 1))
        );
    }

    #[test]
    fn shuffle_is_deterministic_with_seed() {
        let mut a = Queue::with_seed(7);
        let mut b = Queue::with_seed(7);
        a.shuffle = true;
        b.shuffle = true;
        a.play_context(ctx(), tracks(8), Some(0));
        b.play_context(ctx(), tracks(8), Some(0));
        assert_eq!(ids(&a), ids(&b));
    }

    #[test]
    fn previous_restarts_or_goes_back() {
        let mut q = Queue::with_seed(1);
        q.play_context(ctx(), tracks(3), Some(1));
        assert_eq!(q.previous(10_000), Previous::Restart);
        assert_eq!(q.previous(1_000), Previous::Play(t("t0")));
        assert_eq!(q.previous(0), Previous::Restart, "at start without repeat");
        q.repeat = Repeat::Context;
        assert_eq!(q.previous(0), Previous::Play(t("t2")), "wraps with repeat");
    }

    #[test]
    fn unplayable_tracks_are_skipped() {
        let mut ts = tracks(4);
        ts[0].playable = false;
        ts[2].playable = false;
        let mut q = Queue::with_seed(1);
        assert_eq!(q.play_context(ctx(), ts.clone(), Some(0)).unwrap().id, "t1");
        assert_eq!(q.next(true).unwrap().id, "t3");
        assert!(q.next(true).is_none());
        let mut all_bad = ts;
        all_bad[1].playable = false;
        all_bad[3].playable = false;
        assert!(
            Queue::with_seed(1)
                .play_context(ctx(), all_bad, Some(0))
                .is_none()
        );
        assert!(
            Queue::with_seed(1)
                .play_context(ctx(), vec![], None)
                .is_none()
        );
    }

    #[test]
    fn jump_upcoming() {
        let mut q = Queue::with_seed(1);
        q.play_context(ctx(), tracks(5), Some(0));
        assert_eq!(q.jump_upcoming(2).unwrap().id, "t3");
        assert_eq!(ids(&q), vec!["t4"]);
        assert!(q.jump_upcoming(5).is_none());
    }

    #[test]
    fn nav_history() {
        let mut n = Nav::new(Route::Home);
        assert!(!n.can_back());
        assert!(n.push(Route::Search));
        assert!(!n.push(Route::Search), "same route is a no-op");
        n.push(Route::Album("a".into()));
        assert!(n.back());
        assert_eq!(n.current(), &Route::Search);
        assert!(n.can_forward());
        n.push(Route::Liked);
        assert!(!n.can_forward(), "push drops forward history");
        assert!(n.back() && n.back());
        assert_eq!(n.current(), &Route::Home);
        assert!(!n.back());
        for i in 0..100 {
            n.push(Route::Playlist(i.to_string()));
        }
        assert!(n.stack.len() <= NAV_MAX);
    }

    #[test]
    fn route_keys_roundtrip() {
        for r in [
            Route::Home,
            Route::Search,
            Route::Library,
            Route::Liked,
            Route::Playlist("p".into()),
            Route::Album("a".into()),
            Route::Artist("x".into()),
        ] {
            assert_eq!(Route::from_key(&r.key()), Some(r));
        }
        assert_eq!(Route::from_key("bogus"), None);
    }

    #[test]
    fn playback_interpolation() {
        let t0 = Instant::now();
        let mut p = Playback {
            duration_ms: 5000,
            ..Default::default()
        };
        p.report(PlayStatus::Playing, 1000, t0);
        assert_eq!(p.position(t0 + Duration::from_millis(1500)), 2500);
        assert_eq!(p.position(t0 + Duration::from_secs(60)), 5000, "clamped");
        p.report(PlayStatus::Paused, 2000, t0);
        assert_eq!(p.position(t0 + Duration::from_secs(5)), 2000);
        p.seek_preview = Some(4000);
        assert_eq!(p.position(t0), 4000);
        p.muted = true;
        assert_eq!(p.effective_volume(), 0.0);
    }

    #[test]
    fn backoff() {
        assert_eq!(backoff_delay(0, 0.5), Duration::from_millis(1000));
        assert_eq!(backoff_delay(3, 0.5), Duration::from_millis(8000));
        assert_eq!(backoff_delay(30, 1.0), Duration::from_millis(60_000));
        assert!(backoff_delay(2, 0.0) < backoff_delay(2, 0.99));
    }

    #[test]
    fn formatting() {
        assert_eq!(format_duration(187_000), "3:07");
        assert_eq!(format_duration(3_723_000), "1:02:03");
        assert_eq!(format_total(3_780_000), "1 hr 3 min");
        assert_eq!(format_total(2_530_000), "42 min 10 sec");
        assert_eq!(format_count(1_234_567), "1,234,567");
        assert_eq!(format_count(999), "999");
        assert_eq!(Repeat::parse(Repeat::Track.as_str()), Repeat::Track);
    }
}
