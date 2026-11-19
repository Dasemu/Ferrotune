//! Spotify Soloist: Spotify's official headless Spotify Connect player,
//! used as Ferrotune's local audio engine when librespot cannot decrypt
//! audio for the account (audio key code 1, librespot #1649).
//!
//! * `install` downloads the official build from Spotify's CDN on first
//!   use (never bundled) and keeps it updated before it expires;
//! * `process` runs it as a supervised child process;
//! * `ws` / `protocol` talk to its local WebSocket API.
//!
//! The supervisor runs as one tokio task behind a command handle
//! ([`SoloistHandle`]) and an event stream ([`events`]) that the app wraps
//! in an iced `Subscription`, like the playback engine. `--mock` and tests
//! use a simulated engine with the same events. The API key never leaves
//! this module except as the `--api-key` argument / `SOLOIST_API_KEY`
//! variable of the child process.

pub mod archive;
pub mod install;
pub mod process;
pub mod protocol;
pub mod ws;

use std::fmt;
use std::time::{Duration, Instant};

use futures::Stream;
use tokio::process::Child;
use tokio::sync::mpsc;

use crate::auth::secrets::Secret;
use install::{Fetched, InstallMeta, Paths};
use process::LaunchSpec;
use protocol::{Command, Event};

/// Where the user creates their personal Soloist API key.
pub const API_KEY_PAGE: &str = "https://developer.spotify.com/dashboard/soloist";
/// The documentation Ferrotune links to (never to the archives directly).
pub const DOCS_PAGE: &str =
    "https://developer.spotify.com/documentation/soloist/reference/downloads-and-updates";

/// Engine status, as shown in Settings > Playback.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum EngineStatus {
    /// Not set up (no API key saved).
    #[default]
    Off,
    NeedsKey,
    NotInstalled,
    Installing,
    Updating,
    Starting,
    /// Running, but no Spotify Connect session yet: select the device once
    /// from the Spotify app.
    NeedsPairing,
    /// `--pair` is running: waiting for the Spotify app.
    Pairing,
    Ready,
    /// The build expired (exit code 10) and no newer one could be fetched.
    Expired,
    Unsupported,
    Error(String),
}

impl EngineStatus {
    pub fn label(&self) -> String {
        match self {
            Self::Off => "Not set up".into(),
            Self::NeedsKey => "Needs an API key".into(),
            Self::NotInstalled => "Not installed".into(),
            Self::Installing => "Installing…".into(),
            Self::Updating => "Updating…".into(),
            Self::Starting => "Starting…".into(),
            Self::NeedsPairing => "Needs pairing".into(),
            Self::Pairing => "Waiting for the Spotify app…".into(),
            Self::Ready => "Ready".into(),
            Self::Expired => "This build expired".into(),
            Self::Unsupported => "Not available for this computer".into(),
            Self::Error(m) => format!("Error: {m}"),
        }
    }

    pub fn is_ready(&self) -> bool {
        *self == Self::Ready
    }

    /// Running (with or without a session).
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Ready | Self::NeedsPairing)
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Installing | Self::Updating | Self::Starting | Self::Pairing
        )
    }
}

/// Status after Soloist reports its Spotify Connect login.
pub fn status_from_auth(logged_in: bool) -> EngineStatus {
    if logged_in {
        EngineStatus::Ready
    } else {
        EngineStatus::NeedsPairing
    }
}

/// Installed build, for Settings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuildView {
    pub version: Option<String>,
    pub expires_unix: u64,
}

impl From<&InstallMeta> for BuildView {
    fn from(m: &InstallMeta) -> Self {
        Self {
            version: m.version.clone(),
            expires_unix: m.expires_unix(),
        }
    }
}

/// How the daemon is launched (besides the key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchOptions {
    /// The Spotify Connect name (Ferrotune's device name).
    pub device_name: String,
    /// PipeWire node; `None` = default output.
    pub pipewire_device: Option<String>,
    /// `--cache-size` in MB.
    pub cache_mb: u32,
    /// `--initial-volume` 0–100: Ferrotune's last volume.
    pub initial_volume: Option<u8>,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            device_name: crate::config::APP_NAME.to_owned(),
            pipewire_device: None,
            cache_mb: process::DEFAULT_CACHE_MB,
            initial_volume: None,
        }
    }
}

impl LaunchOptions {
    /// Same daemon: the initial volume alone never restarts it (it only
    /// matters at the next start).
    fn same_daemon(&self, other: &Self) -> bool {
        self.device_name == other.device_name
            && self.pipewire_device == other.pipewire_device
            && self.cache_mb == other.cache_mb
    }
}

/// What the app asks of the supervisor.
pub enum Control {
    /// (Re)start with this key and options. `key: None` stops the engine
    /// and reports `NeedsKey`.
    Configure {
        key: Option<Secret>,
        opts: LaunchOptions,
    },
    UpdateNow,
    /// Run `--pair` (replaces the stored session), then restart.
    Pair,
    Ws(Command),
    Shutdown,
}

impl fmt::Debug for Control {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configure { key, .. } => f
                .debug_struct("Configure")
                .field("key", &key.as_ref().map(|_| "[REDACTED]"))
                .finish_non_exhaustive(),
            Self::UpdateNow => f.write_str("UpdateNow"),
            Self::Pair => f.write_str("Pair"),
            Self::Ws(c) => write!(f, "Ws({})", c.name()),
            Self::Shutdown => f.write_str("Shutdown"),
        }
    }
}

#[derive(Clone)]
pub struct SoloistHandle(mpsc::UnboundedSender<Control>);

impl fmt::Debug for SoloistHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SoloistHandle")
    }
}

impl SoloistHandle {
    pub fn send(&self, c: Control) {
        if self.0.send(c).is_err() {
            tracing::debug!("soloist supervisor is gone");
        }
    }

    pub fn command(&self, c: Command) {
        self.send(Control::Ws(c));
    }

    /// Detached handle for tests.
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<Control>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self(tx), rx)
    }
}

#[derive(Debug, Clone)]
pub enum SoloistEvent {
    Ready(SoloistHandle),
    Status(EngineStatus),
    Build(BuildView),
    /// The WebSocket API connected (`true`) or dropped.
    Connected(bool),
    Ws(Event),
    /// Soloist's resident memory in bytes (separate process).
    Memory(Option<u64>),
}

/// Subscription identity: changing it restarts the supervisor.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SoloistKind {
    Mock,
    Real {
        paths: Paths,
        /// Archive URL (Spotify's CDN, or a local test server).
        url: Option<String>,
    },
}

/// Lazily spawns the supervisor when first polled and yields its events.
pub fn events(kind: SoloistKind) -> impl Stream<Item = SoloistEvent> + Send + 'static {
    use futures::StreamExt;
    futures::stream::once(async move {
        let (ctl_tx, ctl_rx) = mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        match kind {
            SoloistKind::Mock => {
                tokio::spawn(run_mock(ctl_rx, ev_tx));
            }
            SoloistKind::Real { paths, url } => {
                tokio::spawn(run(Supervisor::new(paths, url, ev_tx), ctl_rx));
            }
        }
        let ready =
            futures::stream::once(async move { SoloistEvent::Ready(SoloistHandle(ctl_tx)) });
        let rest =
            futures::stream::unfold(
                ev_rx,
                |mut rx| async move { rx.recv().await.map(|e| (e, rx)) },
            );
        ready.chain(rest)
    })
    .flatten()
}

/// Archive URL for this computer, `None` when Spotify has no build for it.
pub fn default_url(base: &str) -> Option<String> {
    install::archive_name(std::env::consts::OS, std::env::consts::ARCH)
        .map(|n| install::archive_url(base, n))
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("Ferrotune/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .timeout(Duration::from_secs(15 * 60))
        .build()
        .unwrap_or_default()
}

/// Messages from the WebSocket task to the supervisor.
enum WsMsg {
    Connected(bool),
    Event(Event),
}

/// A healthy run this long resets the crash backoff.
const HEALTHY: Duration = Duration::from_secs(60);
const STOP_GRACE: Duration = Duration::from_secs(3);
const MEMORY_EVERY: Duration = Duration::from_secs(30);
const UPDATE_EVERY: Duration = Duration::from_secs(3600);

pub struct Supervisor {
    paths: Paths,
    url: Option<String>,
    http: reqwest::Client,
    events: mpsc::UnboundedSender<SoloistEvent>,
    key: Option<Secret>,
    opts: LaunchOptions,
    meta: Option<InstallMeta>,
    status: EngineStatus,
    crashes: u32,
}

/// Why the supervised run of the daemon ended.
enum Stop {
    /// Restart now (new settings, new build, re-pair done).
    Restart,
    /// Wait for the app (no key, unrecoverable error).
    Idle,
    /// Run `--pair`, then restart.
    Pair,
    Shutdown,
}

impl Supervisor {
    pub fn new(
        paths: Paths,
        url: Option<String>,
        events: mpsc::UnboundedSender<SoloistEvent>,
    ) -> Self {
        let meta = InstallMeta::load(&paths).filter(|_| paths.binary().exists());
        Self {
            paths,
            url,
            http: http_client(),
            events,
            key: None,
            opts: LaunchOptions::default(),
            meta,
            status: EngineStatus::Off,
            crashes: 0,
        }
    }

    fn emit(&self, e: SoloistEvent) {
        let _ = self.events.send(e);
    }

    fn set_status(&mut self, s: EngineStatus) {
        if self.status != s {
            tracing::info!(status = ?s, "soloist");
            self.status = s.clone();
            self.emit(SoloistEvent::Status(s));
        }
    }

    fn spec(&self, pair: bool) -> LaunchSpec {
        LaunchSpec {
            binary: self.paths.binary(),
            device_name: self.opts.device_name.clone(),
            data_dir: self.paths.data(),
            cache_dir: self.paths.cache.clone(),
            pipewire_device: self.opts.pipewire_device.clone(),
            cache_mb: self.opts.cache_mb,
            initial_volume: self.opts.initial_volume,
            pair,
        }
    }

    /// Install when missing; `force` asks the CDN for a newer build even
    /// when one is installed. Returns `true` when a new binary was put in.
    async fn ensure_installed(&mut self, force: bool) -> Result<bool, String> {
        let Some(url) = self.url.clone() else {
            self.set_status(EngineStatus::Unsupported);
            return Err("no Soloist build for this computer".into());
        };
        let installed = self.meta.is_some() && self.paths.binary().exists();
        let due = self
            .meta
            .as_ref()
            .is_some_and(|m| install::update_due(m, install::now_unix()));
        if installed && !force && !due {
            return Ok(false);
        }
        self.set_status(if installed {
            EngineStatus::Updating
        } else {
            EngineStatus::Installing
        });
        let current = self.meta.clone().filter(|_| installed);
        match install::fetch(&self.http, &url, &self.paths, current.as_ref()).await {
            Ok(Fetched::Installed(meta)) => {
                tracing::info!(version = ?meta.version, "soloist installed");
                self.emit(SoloistEvent::Build(BuildView::from(&meta)));
                self.meta = Some(meta);
                Ok(true)
            }
            Ok(Fetched::NotModified | Fetched::Older) => {
                if let Some(m) = self.meta.as_mut() {
                    m.checked_unix = install::now_unix();
                    let _ = m.save(&self.paths);
                }
                Ok(false)
            }
            Err(e) if installed => {
                // An update check failing (offline) is not fatal.
                tracing::warn!(error = %e, "soloist update check failed");
                Ok(false)
            }
            Err(e) => Err(e.user_message_for_download()),
        }
    }
}

trait DownloadMessage {
    fn user_message_for_download(&self) -> String;
}

impl DownloadMessage for crate::error::Error {
    fn user_message_for_download(&self) -> String {
        use crate::error::Error;
        match self {
            Error::Net(_) => "could not download Spotify Soloist (offline?)".into(),
            Error::Http { status, .. } => {
                format!("Spotify's download server answered {status}")
            }
            Error::Protocol(m) => m.clone(),
            other => other.user_message(),
        }
    }
}

async fn run(mut sup: Supervisor, mut ctl: mpsc::UnboundedReceiver<Control>) {
    if let Some(m) = &sup.meta {
        sup.emit(SoloistEvent::Build(BuildView::from(m)));
    }
    let mut idle = true;
    loop {
        if idle {
            // Wait for the app (Configure / Pair / UpdateNow).
            let Some(c) = ctl.recv().await else { return };
            match c {
                Control::Configure { key, opts } => {
                    sup.opts = opts;
                    sup.key = key;
                    sup.crashes = 0;
                }
                Control::Shutdown => return,
                Control::Ws(c) => {
                    tracing::debug!(command = c.name(), "soloist not running; dropped");
                    continue;
                }
                Control::UpdateNow | Control::Pair => {}
            }
            if sup.key.is_none() {
                sup.set_status(EngineStatus::NeedsKey);
                continue;
            }
            idle = false;
        }
        let mut stop = run_daemon(&mut sup, &mut ctl).await;
        loop {
            match stop {
                Stop::Pair => stop = run_pair(&mut sup, &mut ctl).await,
                Stop::Restart => break,
                Stop::Idle => {
                    idle = true;
                    break;
                }
                Stop::Shutdown => return,
            }
        }
    }
}

/// One supervised run: install/update, spawn, watch, until it must stop.
async fn run_daemon(sup: &mut Supervisor, ctl: &mut mpsc::UnboundedReceiver<Control>) -> Stop {
    let Some(key) = sup.key.clone() else {
        sup.set_status(EngineStatus::NeedsKey);
        return Stop::Idle;
    };
    if let Err(m) = sup.ensure_installed(false).await {
        if sup.status != EngineStatus::Unsupported {
            sup.set_status(EngineStatus::Error(m));
        }
        return wait_or_retry(sup, ctl).await;
    }
    let data = sup.paths.data();
    process::stop_stale(&data, &sup.paths.binary()).await;
    ws::clear_endpoint(&data);
    sup.set_status(EngineStatus::Starting);
    let mut child = match process::spawn(&sup.spec(false), &key) {
        Ok(c) => c,
        Err(e) => {
            sup.set_status(EngineStatus::Error(format!("could not start Soloist: {e}")));
            return wait_or_retry(sup, ctl).await;
        }
    };
    let started = Instant::now();
    pipe_child_logs(&mut child, &key);
    let pid = child.id();

    let (ws_cmd_tx, ws_cmd_rx) = mpsc::unbounded_channel::<Command>();
    let (ws_ev_tx, mut ws_ev_rx) = mpsc::unbounded_channel::<WsMsg>();
    let ws_task = tokio::spawn(ws_loop(data.clone(), ws_cmd_rx, ws_ev_tx));
    let mut memory = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(5),
        MEMORY_EVERY,
    );
    let mut update =
        tokio::time::interval_at(tokio::time::Instant::now() + UPDATE_EVERY, UPDATE_EVERY);

    let stop = loop {
        tokio::select! {
            c = ctl.recv() => match c {
                None | Some(Control::Shutdown) => break Stop::Shutdown,
                Some(Control::Configure { key, opts }) => {
                    let unchanged = sup.opts.same_daemon(&opts)
                        && sup.key.as_ref().map(Secret::expose) == key.as_ref().map(Secret::expose);
                    sup.opts = opts;
                    sup.key = key;
                    if unchanged {
                        continue;
                    }
                    break if sup.key.is_some() { Stop::Restart } else { Stop::Idle };
                }
                Some(Control::Ws(c)) => {
                    let _ = ws_cmd_tx.send(c);
                }
                Some(Control::UpdateNow) => {
                    let before = sup.status.clone();
                    match sup.ensure_installed(true).await {
                        Ok(true) => break Stop::Restart,
                        Ok(false) => sup.set_status(before),
                        Err(m) => {
                            tracing::warn!(error = %m, "soloist update failed");
                            sup.set_status(before);
                        }
                    }
                }
                Some(Control::Pair) => break Stop::Pair,
            },
            st = child.wait() => {
                let code = st.ok().and_then(|s| s.code());
                ws_task.abort();
                sup.emit(SoloistEvent::Connected(false));
                sup.emit(SoloistEvent::Memory(None));
                if started.elapsed() >= HEALTHY {
                    sup.crashes = 0;
                }
                return on_exit(sup, ctl, code).await;
            }
            m = ws_ev_rx.recv() => match m {
                Some(WsMsg::Connected(on)) => {
                    sup.emit(SoloistEvent::Connected(on));
                    if !on && sup.status.is_running() {
                        sup.set_status(EngineStatus::Starting);
                    }
                }
                Some(WsMsg::Event(ev)) => {
                    if let Event::AuthState { logged_in, .. } = &ev {
                        sup.set_status(status_from_auth(*logged_in));
                    }
                    sup.emit(SoloistEvent::Ws(ev));
                }
                None => {}
            },
            _ = memory.tick() => {
                sup.emit(SoloistEvent::Memory(pid.and_then(process::rss_bytes)));
            }
            _ = update.tick() => {
                if sup.meta.as_ref().is_some_and(|m| install::update_due(m, install::now_unix())) {
                    let before = sup.status.clone();
                    match sup.ensure_installed(false).await {
                        // Swap builds when nothing is playing would be nicer;
                        // a restart takes a second and keeps the session.
                        Ok(true) => break Stop::Restart,
                        _ => sup.set_status(before),
                    }
                }
            }
        }
    };
    stop_child(&mut child, &ws_task).await;
    sup.emit(SoloistEvent::Connected(false));
    sup.emit(SoloistEvent::Memory(None));
    if matches!(stop, Stop::Idle) {
        sup.set_status(EngineStatus::NeedsKey);
    }
    stop
}

async fn stop_child(child: &mut Child, ws_task: &tokio::task::JoinHandle<()>) {
    ws_task.abort();
    let _ = process::terminate(child, STOP_GRACE).await;
}

fn pipe_child_logs(child: &mut Child, key: &Secret) {
    if let Some(out) = child.stdout.take() {
        tokio::spawn(process::pipe_logs(out, key.clone(), "stdout"));
    }
    if let Some(err) = child.stderr.take() {
        tokio::spawn(process::pipe_logs(err, key.clone(), "stderr"));
    }
}

/// The daemon exited on its own.
async fn on_exit(
    sup: &mut Supervisor,
    ctl: &mut mpsc::UnboundedReceiver<Control>,
    code: Option<i32>,
) -> Stop {
    tracing::warn!(?code, "soloist exited");
    if code == Some(process::EXIT_EXPIRED) {
        sup.set_status(EngineStatus::Updating);
        return match sup.ensure_installed(true).await {
            Ok(true) => Stop::Restart,
            _ => {
                sup.set_status(EngineStatus::Expired);
                wait_or_retry(sup, ctl).await
            }
        };
    }
    sup.set_status(EngineStatus::Error(match code {
        Some(c) => format!("Soloist stopped (exit code {c})"),
        None => "Soloist stopped".into(),
    }));
    wait_or_retry(sup, ctl).await
}

/// Back off, then restart; any app command wins over the timer.
async fn wait_or_retry(sup: &mut Supervisor, ctl: &mut mpsc::UnboundedReceiver<Control>) -> Stop {
    if sup.status == EngineStatus::Unsupported {
        return Stop::Idle;
    }
    let delay = process::backoff(sup.crashes);
    sup.crashes = sup.crashes.saturating_add(1);
    let sleep = tokio::time::sleep(delay);
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            () = &mut sleep => return Stop::Restart,
            c = ctl.recv() => match c {
                None | Some(Control::Shutdown) => return Stop::Shutdown,
                Some(Control::Configure { key, opts }) => {
                    sup.opts = opts;
                    sup.key = key;
                    sup.crashes = 0;
                    return if sup.key.is_some() { Stop::Restart } else { Stop::Idle };
                }
                Some(Control::UpdateNow) => {
                    sup.crashes = 0;
                    return match sup.ensure_installed(true).await {
                        Ok(_) => Stop::Restart,
                        Err(m) => {
                            sup.set_status(EngineStatus::Error(m));
                            continue;
                        }
                    };
                }
                Some(Control::Pair) => return Stop::Pair,
                Some(Control::Ws(_)) => {}
            },
        }
    }
}

/// `--pair`: Soloist waits for the Spotify app, stores the new session and
/// exits. The daemon is restarted afterwards either way.
async fn run_pair(sup: &mut Supervisor, ctl: &mut mpsc::UnboundedReceiver<Control>) -> Stop {
    let Some(key) = sup.key.clone() else {
        sup.set_status(EngineStatus::NeedsKey);
        return Stop::Idle;
    };
    if sup.ensure_installed(false).await.is_err() {
        return wait_or_retry(sup, ctl).await;
    }
    let mut child = match process::spawn(&sup.spec(true), &key) {
        Ok(c) => c,
        Err(e) => {
            sup.set_status(EngineStatus::Error(format!("could not start Soloist: {e}")));
            return wait_or_retry(sup, ctl).await;
        }
    };
    pipe_child_logs(&mut child, &key);
    sup.set_status(EngineStatus::Pairing);
    loop {
        tokio::select! {
            st = child.wait() => {
                let code = st.ok().and_then(|s| s.code());
                tracing::info!(?code, "soloist pairing finished");
                if code == Some(process::EXIT_EXPIRED) {
                    return on_exit(sup, ctl, code).await;
                }
                return Stop::Restart;
            }
            c = ctl.recv() => match c {
                None | Some(Control::Shutdown) => {
                    let _ = process::terminate(&mut child, STOP_GRACE).await;
                    return Stop::Shutdown;
                }
                // Cancel pairing: back to the normal daemon.
                Some(Control::Configure { key, opts }) => {
                    sup.opts = opts;
                    sup.key = key;
                    let _ = process::terminate(&mut child, STOP_GRACE).await;
                    return if sup.key.is_some() { Stop::Restart } else { Stop::Idle };
                }
                Some(_) => {}
            },
        }
    }
}

/// Find the endpoint, connect, and reconnect while the daemon runs.
async fn ws_loop(
    data: std::path::PathBuf,
    mut cmds: mpsc::UnboundedReceiver<Command>,
    out: mpsc::UnboundedSender<WsMsg>,
) {
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<Event>();
    // Forward parsed events.
    let fwd = {
        let out = out.clone();
        tokio::spawn(async move {
            while let Some(e) = ev_rx.recv().await {
                if out.send(WsMsg::Event(e)).is_err() {
                    return;
                }
            }
        })
    };
    loop {
        let Some(addr) = ws::discover(&data, Duration::from_secs(20)).await else {
            tracing::debug!("soloist WebSocket endpoint not found yet");
            if out.is_closed() {
                break;
            }
            continue;
        };
        tracing::debug!("soloist WebSocket found");
        let _ = out.send(WsMsg::Connected(true));
        let r = ws::run(addr, &mut cmds, &ev_tx).await;
        if out.send(WsMsg::Connected(false)).is_err() {
            break;
        }
        if let Err(e) = r {
            tracing::debug!(error = %e, "soloist WebSocket closed");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    fwd.abort();
}

/// Simulated engine for `--mock`: walks through the statuses and answers
/// commands with the events Soloist documents.
pub struct MockSoloist {
    pub status: EngineStatus,
    pub snapshot: protocol::PlaybackSnapshot,
}

impl Default for MockSoloist {
    fn default() -> Self {
        Self {
            status: EngineStatus::Off,
            snapshot: protocol::PlaybackSnapshot {
                volume: 70.0,
                ..Default::default()
            },
        }
    }
}

impl MockSoloist {
    pub fn command(&mut self, c: &Command, now_ms: u64) -> Vec<Event> {
        let s = &mut self.snapshot;
        let anchor = |s: &protocol::PlaybackSnapshot, speed: f64| protocol::Position {
            position_ms: f64::from(s.position.at(now_ms)),
            timestamp_ms: now_ms as f64,
            speed,
        };
        let mut out = vec![];
        match c {
            Command::Play(uri) => {
                if let Some(uri) = uri {
                    s.item = Some(protocol::Entity {
                        uri: uri.clone(),
                        entity_type: "track".into(),
                        ..Default::default()
                    });
                    s.position = protocol::Position::default();
                    out.push(Event::TrackChanged {
                        item: s.item.clone().unwrap_or_default(),
                    });
                }
                s.status = protocol::Status::Playing;
                s.is_active = true;
                s.position = anchor(s, 1.0);
                out.push(Event::PlaybackChanged { status: s.status });
                out.push(Event::PositionSync {
                    position: s.position,
                });
            }
            Command::Pause => {
                s.status = protocol::Status::Paused;
                s.position = anchor(s, 0.0);
                out.push(Event::PlaybackChanged { status: s.status });
                out.push(Event::PositionSync {
                    position: s.position,
                });
            }
            Command::Seek(ms) => {
                let speed = if s.status == protocol::Status::Playing {
                    1.0
                } else {
                    0.0
                };
                s.position = protocol::Position {
                    position_ms: f64::from(*ms),
                    timestamp_ms: now_ms as f64,
                    speed,
                };
                out.push(Event::PositionSync {
                    position: s.position,
                });
            }
            Command::SetVolume(v) => {
                s.volume = f64::from(*v);
                out.push(Event::VolumeChanged { volume: s.volume });
            }
            Command::SetShuffle(on) => {
                s.options.shuffle = *on;
                out.push(Event::OptionsChanged {
                    options: s.options.clone(),
                });
            }
            Command::SetRepeatContext(on) | Command::SetRepeatTrack(on) => {
                let track = matches!(c, Command::SetRepeatTrack(_));
                s.options.repeat = match (track, on, s.options.repeat) {
                    (true, true, _) => protocol::RepeatMode::Track,
                    (false, true, _) => protocol::RepeatMode::Context,
                    (true, false, protocol::RepeatMode::Track)
                    | (false, false, protocol::RepeatMode::Context) => protocol::RepeatMode::Off,
                    (_, false, m) => m,
                };
                out.push(Event::OptionsChanged {
                    options: s.options.clone(),
                });
            }
            Command::Activate => {
                s.is_active = true;
                out.push(Event::DeviceChanged {
                    is_active: true,
                    device_name: String::new(),
                });
            }
            Command::Deactivate => {
                s.is_active = false;
                out.push(Event::DeviceChanged {
                    is_active: false,
                    device_name: String::new(),
                });
            }
            Command::GetState => out.push(Event::PlaybackState(Box::new(s.clone()))),
            _ => {}
        }
        if !matches!(
            c,
            Command::GetState | Command::GetAuthState | Command::GetQueue(_)
        ) {
            out.insert(
                0,
                Event::CommandResult {
                    command: c.name().into(),
                },
            );
        }
        out
    }
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

async fn run_mock(
    mut ctl: mpsc::UnboundedReceiver<Control>,
    ev: mpsc::UnboundedSender<SoloistEvent>,
) {
    let mut m = MockSoloist::default();
    let send = |e: SoloistEvent| ev.send(e).is_ok();
    let mut device_name = String::new();
    while let Some(c) = ctl.recv().await {
        match c {
            Control::Configure { key, opts } => {
                device_name = opts.device_name;
                if key.is_none() {
                    m.status = EngineStatus::NeedsKey;
                    send(SoloistEvent::Status(m.status.clone()));
                    continue;
                }
                for s in [
                    EngineStatus::Installing,
                    EngineStatus::Starting,
                    EngineStatus::Ready,
                ] {
                    m.status = s.clone();
                    if !send(SoloistEvent::Status(s)) {
                        return;
                    }
                }
                send(SoloistEvent::Build(BuildView {
                    version: Some("1.0.0".into()),
                    expires_unix: install::now_unix() + 80 * archive::DAY_SECS,
                }));
                send(SoloistEvent::Connected(true));
                send(SoloistEvent::Ws(Event::AuthState {
                    logged_in: true,
                    is_active: false,
                    device_name: device_name.clone(),
                }));
                send(SoloistEvent::Memory(Some(38 * 1024 * 1024)));
            }
            Control::Ws(c) => {
                for e in m.command(&c, unix_ms()) {
                    if !send(SoloistEvent::Ws(e)) {
                        return;
                    }
                }
            }
            Control::Pair => {
                send(SoloistEvent::Status(EngineStatus::Pairing));
                send(SoloistEvent::Status(EngineStatus::Ready));
            }
            Control::UpdateNow => {
                send(SoloistEvent::Status(EngineStatus::Updating));
                send(SoloistEvent::Status(m.status.clone()));
            }
            Control::Shutdown => return,
        }
    }
    let _ = device_name;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_labels_and_predicates() {
        assert_eq!(status_from_auth(true), EngineStatus::Ready);
        assert_eq!(status_from_auth(false), EngineStatus::NeedsPairing);
        assert!(EngineStatus::Ready.is_ready());
        assert!(EngineStatus::NeedsPairing.is_running());
        assert!(!EngineStatus::NeedsKey.is_running());
        assert!(EngineStatus::Installing.is_busy());
        assert_eq!(
            EngineStatus::Error("x".into()).label(),
            "Error: x".to_owned()
        );
    }

    #[test]
    fn initial_volume_alone_does_not_restart() {
        let a = LaunchOptions::default();
        let b = LaunchOptions {
            initial_volume: Some(80),
            ..a.clone()
        };
        assert!(a.same_daemon(&b));
        let c = LaunchOptions {
            cache_mb: 1000,
            ..a.clone()
        };
        assert!(!a.same_daemon(&c));
    }

    #[test]
    fn control_debug_hides_the_key() {
        let c = Control::Configure {
            key: Some(Secret::new("super-secret-key")),
            opts: LaunchOptions::default(),
        };
        let s = format!("{c:?}");
        assert!(!s.contains("super-secret"), "{s}");
    }

    #[test]
    fn mock_answers_like_the_reference() {
        let mut m = MockSoloist::default();
        let ev = m.command(&Command::Play(Some("spotify:track:x".into())), 1000);
        assert_eq!(
            ev[0],
            Event::CommandResult {
                command: "play".into()
            }
        );
        assert!(ev.contains(&Event::PlaybackChanged {
            status: protocol::Status::Playing
        }));
        m.command(&Command::Pause, 3000);
        assert_eq!(m.snapshot.position.at(9999), 2000);
        for c in protocol::repeat_commands(protocol::RepeatMode::Track) {
            m.command(&c, 0);
        }
        assert_eq!(m.snapshot.options.repeat, protocol::RepeatMode::Track);
        for c in protocol::repeat_commands(protocol::RepeatMode::Off) {
            m.command(&c, 0);
        }
        assert_eq!(m.snapshot.options.repeat, protocol::RepeatMode::Off);
    }
}
