//! Application state and the single `update` mutation point (Elm style).
//! No I/O happens here directly: side effects are returned as `Task`s that
//! call into `net`, `store` and `auth`.

pub mod images;
pub mod message;
pub mod state;

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::{Subscription, Task};

pub use message::{Done, Message, Net, SessionReady, TrackSource, Ui, ui};
use state::{
    Connection, LibraryFilter, Nav, PlayContext, PlayStatus, Playback, Previous, Queue, Repeat,
    Route, SearchFilter, backoff_delay,
};

use crate::auth::oauth::{self, TokenSet};
use crate::auth::secrets::{KEY_LIBRESPOT, KEY_OAUTH, KeyringStore, MemoryStore, SecretStore};
use crate::auth::{AuthEvent, AuthState};
use crate::config::{self, Config};
use crate::error::{Error, Result};
use crate::net::DataSource;
use crate::net::model::{
    Album, ArtistPage, HomeFeed, Library, Page, Playlist, SearchResults, Track, TrackList, User,
    pick_image,
};
use crate::net::mpris::{
    MediaCommand, MediaEvent, MediaHandle, MediaLoop, MediaSnapshot, MediaStatus, MediaTrack,
};
use crate::net::playback::{
    self, EngineKind, PlaybackCommand, PlaybackEvent, PlaybackHandle, SessionCredentials,
    SpotifySession, StoredCredentials,
};
use crate::net::webapi::{TokenManager, WebApi, http_client};
use crate::store::{Kind, Store};

/// Max images requested per screen (rows beyond this load lazily later).
const MAX_IMAGE_REQUESTS: usize = 80;
const TOAST_SECS: u64 = 5;
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct ScreenshotPlan {
    pub screen: String,
    pub out: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub screenshot: Option<ScreenshotPlan>,
    pub started: Option<Instant>,
}

#[derive(Debug, Default)]
pub struct SearchState {
    pub query: String,
    pub results: Option<SearchResults>,
    pub loading: bool,
    pub filter: SearchFilter,
    pub seq: u64,
    pub error: Option<String>,
}

pub struct App {
    pub config: Arc<Config>,
    pub secrets: Arc<dyn SecretStore>,
    pub store: Store,
    pub source: Option<DataSource>,
    pub auth: AuthState,
    pub user: Option<User>,
    pub connection: Connection,
    pub nav: Nav,
    pub library: Library,
    pub library_filter: LibraryFilter,
    pub home: Option<HomeFeed>,
    pub playlists: HashMap<String, (Playlist, TrackList)>,
    pub albums: HashMap<String, Album>,
    pub artists: HashMap<String, ArtistPage>,
    pub liked: Option<TrackList>,
    pub liked_ids: HashSet<String>,
    pub loading: HashSet<Route>,
    pub errors: HashMap<Route, String>,
    pub search: SearchState,
    pub queue: Queue,
    pub playback: Playback,
    pub engine_kind: EngineKind,
    pub engine: Option<PlaybackHandle>,
    pub session: Option<SpotifySession>,
    pub media: Option<MediaHandle>,
    pub mpris_enabled: bool,
    last_media: Option<MediaSnapshot>,
    pub images: images::ImageCache,
    image_requests: HashSet<String>,
    pub show_queue: bool,
    pub hovered_row: Option<String>,
    pub selected_row: Option<String>,
    pub toast: Option<(String, Instant)>,
    pub now: Instant,
    login_abort: Option<iced::task::Handle>,
    loading_more: bool,
    audio_attempt: u32,
    /// The engine lost the loaded track (session died): reload on play.
    reload_on_play: bool,
    screenshot: Option<ScreenshotPlan>,
    started: Instant,
    pub first_frame_logged: Cell<bool>,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("auth", &self.auth)
            .field("route", self.nav.current())
            .finish_non_exhaustive()
    }
}

fn store_write<T>(store: &Store, kind: Kind, id: String, value: T) -> Task<Message>
where
    T: serde::Serialize + Send + 'static,
{
    let store = store.clone();
    Task::perform(
        async move {
            let r = store
                .run(move |s| {
                    s.put(kind, &id, &value)?;
                    s.prune(crate::store::MAX_ENTITIES).map(|_| ())
                })
                .await;
            if let Err(e) = r {
                tracing::warn!(error = %e, "cache write failed");
            }
        },
        |()| Message::Done(Done::Noop),
    )
}

fn store_read<T, F>(store: &Store, kind: Kind, id: String, f: F) -> Task<Message>
where
    T: serde::de::DeserializeOwned + Send + 'static,
    F: Fn(T) -> Done + Send + 'static,
{
    let store = store.clone();
    Task::perform(
        async move { store.run(move |s| s.get::<T>(kind, &id)).await },
        move |r| match r {
            Ok(Some(v)) => Message::Done(f(v)),
            _ => Message::Done(Done::Noop),
        },
    )
}

/// Persist refreshed tokens to the keyring off the async executor.
fn persist_tokens(secrets: Arc<dyn SecretStore>) -> crate::net::webapi::RefreshCallback {
    Arc::new(move |t: &TokenSet| {
        let secrets = secrets.clone();
        match t.to_secret() {
            Ok(secret) => {
                tokio::task::spawn_blocking(move || {
                    if let Err(e) = secrets.set(KEY_OAUTH, &secret) {
                        tracing::warn!(error = %e, "could not store refreshed token");
                    }
                });
            }
            Err(e) => tracing::warn!(error = %e, "could not serialise token"),
        }
    })
}

fn build_source(config: &Config, secrets: Arc<dyn SecretStore>, tokens: TokenSet) -> DataSource {
    let http = http_client();
    let manager = TokenManager::new(
        tokens,
        config.client_id.clone().unwrap_or_default(),
        config.accounts_base.clone(),
        http.clone(),
        Some(persist_tokens(secrets)),
    );
    DataSource::Web(WebApi::new(
        http,
        config.api_base.clone(),
        Arc::new(manager),
    ))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| Error::Cancelled)?
}

async fn restore_session(
    config: Arc<Config>,
    secrets: Arc<dyn SecretStore>,
) -> Result<Option<SessionReady>> {
    let s = secrets.clone();
    let Some(secret) = blocking(move || s.get(KEY_OAUTH)).await? else {
        return Ok(None);
    };
    let tokens = match TokenSet::from_secret(&secret) {
        Ok(t) => t,
        Err(_) => {
            let s = secrets.clone();
            blocking(move || s.delete(KEY_OAUTH)).await?;
            return Ok(None);
        }
    };
    Ok(Some(SessionReady {
        source: build_source(&config, secrets, tokens),
        user: None,
    }))
}

async fn login_flow(
    config: Arc<Config>,
    secrets: Arc<dyn SecretStore>,
    listener: std::net::TcpListener,
    state: String,
    verifier: crate::auth::secrets::Secret,
) -> Result<SessionReady> {
    let client_id = config
        .client_id
        .clone()
        .ok_or_else(|| Error::auth("No client id configured."))?;
    let code = oauth::wait_for_callback(listener, state, LOGIN_TIMEOUT).await?;
    let http = http_client();
    let tokens = oauth::exchange_code(
        &http,
        &config.accounts_base,
        &client_id,
        &code,
        &config.redirect_uri(),
        &verifier,
    )
    .await?;
    let secret = tokens.to_secret()?;
    let s = secrets.clone();
    blocking(move || s.set(KEY_OAUTH, &secret)).await?;
    let source = build_source(&config, secrets, tokens);
    let user = source.me().await.ok();
    Ok(SessionReady { source, user })
}

async fn connect_audio(
    source: DataSource,
    secrets: Arc<dyn SecretStore>,
) -> Result<SpotifySession> {
    let DataSource::Web(api) = source else {
        return Err(Error::Playback("mock mode has no audio session".into()));
    };
    let s = secrets.clone();
    let stored = blocking(move || s.get(KEY_LIBRESPOT)).await?;
    let mut result = None;
    if let Some(stored) = stored.and_then(|s| StoredCredentials::from_secret(&s).ok()) {
        match playback::connect_session(SessionCredentials::Stored(stored)).await {
            Ok(r) => result = Some(r),
            Err(e) => tracing::warn!(error = %e, "stored audio credentials rejected; using token"),
        }
    }
    let (session, creds) = match result {
        Some(r) => r,
        None => {
            let token = api.tokens().access_token().await?;
            playback::connect_session(SessionCredentials::AccessToken(token)).await?
        }
    };
    let secret = creds.to_secret()?;
    blocking(move || secrets.set(KEY_LIBRESPOT, &secret)).await?;
    Ok(session)
}

fn playback_stream(kind: &EngineKind) -> impl futures::Stream<Item = PlaybackEvent> + use<> {
    playback::events(*kind)
}

fn media_stream() -> impl futures::Stream<Item = MediaEvent> {
    crate::net::mpris::events()
}

impl App {
    pub fn boot(config: Config, opts: Options) -> (Self, Task<Message>) {
        let config = Arc::new(config);
        let mock = config.mock;
        let secrets: Arc<dyn SecretStore> = if mock {
            Arc::new(MemoryStore::new())
        } else {
            Arc::new(KeyringStore::default())
        };
        let mut toast = None;
        let store = if mock {
            Store::in_memory()
        } else {
            Store::open(&config.db_path())
        }
        .or_else(|e| {
            tracing::warn!(error = %e, "falling back to an in-memory cache");
            toast = Some(e.user_message());
            Store::in_memory()
        });
        let store = match store {
            Ok(s) => s,
            Err(e) => {
                // Opening an in-memory database cannot realistically fail;
                // if it does, there is nothing sensible to run.
                tracing::error!(error = %e, "no SQLite available");
                std::process::exit(1);
            }
        };

        // First frame comes from the cache: no network on the critical path.
        let library: Library = store
            .get(Kind::Library, "me")
            .ok()
            .flatten()
            .unwrap_or_default();
        let home: Option<HomeFeed> = store.get(Kind::Home, "me").ok().flatten();
        let cached_user: Option<User> = store.get(Kind::User, "me").ok().flatten();
        let setting = |k: &str| store.setting(k).ok().flatten();
        let mut playback_state = Playback::default();
        if let Some(v) = setting("volume").and_then(|v| v.parse::<f32>().ok()) {
            playback_state.volume = v.clamp(0.0, 1.0);
        }
        let mut queue = Queue::default();
        queue.shuffle = setting("shuffle").as_deref() == Some("1");
        queue.repeat = setting("repeat")
            .map(|r| Repeat::parse(&r))
            .unwrap_or_default();
        let start_route = setting("route")
            .and_then(|r| Route::from_key(&r))
            .unwrap_or(Route::Home);

        let screenshot = opts.screenshot.clone();
        let mut app = Self {
            auth: AuthState::initial(config.client_id.is_some() || mock),
            engine_kind: if mock {
                EngineKind::Mock
            } else {
                EngineKind::Librespot(config.bitrate.into())
            },
            mpris_enabled: screenshot.is_none() && cfg!(target_os = "linux"),
            config,
            secrets,
            store,
            source: None,
            user: cached_user,
            connection: Connection::Disconnected,
            nav: Nav::new(start_route),
            library,
            library_filter: LibraryFilter::All,
            home,
            playlists: HashMap::new(),
            albums: HashMap::new(),
            artists: HashMap::new(),
            liked: None,
            liked_ids: HashSet::new(),
            loading: HashSet::new(),
            errors: HashMap::new(),
            search: SearchState::default(),
            queue,
            playback: playback_state,
            engine: None,
            session: None,
            media: None,
            last_media: None,
            images: images::ImageCache::new(images::DEFAULT_BUDGET),
            image_requests: HashSet::new(),
            show_queue: false,
            hovered_row: None,
            selected_row: None,
            toast: toast.map(|t| (t, Instant::now())),
            now: Instant::now(),
            login_abort: None,
            loading_more: false,
            audio_attempt: 0,
            reload_on_play: false,
            screenshot,
            started: opts.started.unwrap_or_else(Instant::now),
            first_frame_logged: Cell::new(false),
        };

        let mut tasks = Vec::new();
        if mock {
            tasks.push(app.signed_in(SessionReady {
                source: DataSource::Mock,
                user: Some(crate::mock::user()),
            }));
            if let Some(plan) = app.screenshot.clone() {
                tasks.push(app.prepare_screenshot(&plan));
            }
        } else if app.config.client_id.is_some() {
            if let Err(e) = KeyringStore::check_available() {
                app.auth = AuthState::SignedOut {
                    error: Some(e.user_message()),
                };
            } else {
                let (config, secrets) = (app.config.clone(), app.secrets.clone());
                tasks.push(Task::perform(restore_session(config, secrets), |r| {
                    Message::Done(Done::Restored(r))
                }));
            }
        }
        tasks.push(app.request_images());
        (app, Task::batch(tasks))
    }

    pub fn title(&self) -> String {
        match &self.queue.current {
            Some(t) if self.playback.is_playing() => format!("{} • {}", t.name, t.artist_line()),
            _ => config::APP_NAME.to_owned(),
        }
    }

    pub fn route(&self) -> &Route {
        self.nav.current()
    }

    pub fn is_mock(&self) -> bool {
        self.config.mock
    }

    /// Log time-to-first-frame once (FERRO_TRACE_STARTUP=1).
    pub fn mark_frame(&self) {
        if !self.first_frame_logged.get() {
            self.first_frame_logged.set(true);
            if self.config.trace_startup {
                let ms = self.started.elapsed().as_secs_f64() * 1000.0;
                tracing::info!(first_frame_ms = format!("{ms:.1}"), "first frame");
                eprintln!("first_frame_ms={ms:.1}");
            }
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            Subscription::run_with(self.engine_kind, playback_stream)
                .map(|e| Message::Net(Net::Playback(e))),
        ];
        if self.mpris_enabled {
            subs.push(Subscription::run(media_stream).map(|e| Message::Net(Net::Media(e))));
        }
        if self.playback.status == PlayStatus::Playing || self.toast.is_some() {
            subs.push(
                iced::time::every(Duration::from_millis(500)).map(|i| Message::Ui(Ui::Tick(i))),
            );
        }
        Subscription::batch(subs)
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let is_tick = matches!(message, Message::Ui(Ui::Tick(_)));
        self.now = Instant::now();
        let task = match message {
            Message::Ui(u) => self.update_ui(u),
            Message::Net(Net::Playback(e)) => self.on_playback(e),
            Message::Net(Net::Media(e)) => self.on_media(e),
            Message::Done(d) => self.on_done(d),
        };
        if is_tick {
            return task;
        }
        self.push_media_snapshot();
        Task::batch([task, self.request_images()])
    }

    // ----------------------------------------------------------------- auth

    fn signed_in(&mut self, ready: SessionReady) -> Task<Message> {
        let user = ready
            .user
            .clone()
            .or_else(|| self.user.clone())
            .unwrap_or_else(|| User {
                id: String::new(),
                display_name: String::new(),
                image: None,
                product: None,
            });
        self.auth = self.auth.apply(AuthEvent::Connected(user.clone()));
        self.user = Some(user);
        self.source = Some(ready.source.clone());
        self.connection = if self.is_mock() {
            Connection::Connected
        } else {
            Connection::Connecting
        };
        let mut tasks = vec![self.fetch_library(), self.fetch_home(), self.load_route()];
        match ready.user {
            Some(u) => tasks.push(store_write(&self.store, Kind::User, "me".into(), u)),
            None => {
                let src = ready.source.clone();
                tasks.push(Task::perform(async move { src.me().await }, |r| {
                    Message::Done(Done::Profile(r))
                }));
            }
        }
        if !self.is_mock() {
            tasks.push(self.connect_audio_task());
        }
        Task::batch(tasks)
    }

    fn connect_audio_task(&self) -> Task<Message> {
        let Some(source) = self.source.clone() else {
            return Task::none();
        };
        let secrets = self.secrets.clone();
        Task::perform(connect_audio(source, secrets), |r| {
            Message::Done(Done::Audio(r))
        })
    }

    fn start_login(&mut self) -> Task<Message> {
        let Some(client_id) = self.config.client_id.clone() else {
            self.auth = AuthState::MissingClientId;
            return Task::none();
        };
        let pkce = oauth::Pkce::generate();
        let state = oauth::random_state();
        let url = match oauth::authorize_url(
            &self.config.accounts_base,
            &client_id,
            &self.config.redirect_uri(),
            &pkce.challenge,
            &state,
            config::SCOPES,
        ) {
            Ok(u) => u,
            Err(e) => {
                self.auth = self.auth.apply(AuthEvent::Failed(e));
                return Task::none();
            }
        };
        if let Some(h) = self.login_abort.take() {
            h.abort();
        }
        // Bind first so the browser redirect can never beat the listener.
        let listener = match oauth::bind_callback(self.config.redirect_port) {
            Ok(l) => l,
            Err(e) => {
                self.auth = self.auth.apply(AuthEvent::Failed(e));
                return Task::none();
            }
        };
        if let Err(e) = oauth::open_browser(&url) {
            tracing::warn!(error = %e, "could not open browser; showing URL");
        }
        self.auth = self.auth.apply(AuthEvent::LoginStarted { url });
        let (task, handle) = Task::perform(
            login_flow(
                self.config.clone(),
                self.secrets.clone(),
                listener,
                state,
                pkce.verifier,
            ),
            |r| Message::Done(Done::LoggedIn(r)),
        )
        .abortable();
        self.login_abort = Some(handle);
        task
    }

    fn logout(&mut self) -> Task<Message> {
        if let Some(e) = &self.engine {
            e.send(PlaybackCommand::Stop);
        }
        if let Some(s) = self.session.take() {
            s.0.shutdown();
        }
        self.source = None;
        self.user = None;
        self.queue.clear();
        self.playback = Playback {
            volume: self.playback.volume,
            ..Playback::default()
        };
        self.library = Library::default();
        self.home = None;
        self.playlists.clear();
        self.albums.clear();
        self.artists.clear();
        self.liked = None;
        self.liked_ids.clear();
        self.search = SearchState::default();
        self.connection = Connection::Disconnected;
        self.nav = Nav::new(Route::Home);
        self.auth = self.auth.apply(AuthEvent::LoggedOut);
        let secrets = self.secrets.clone();
        let store = self.store.clone();
        Task::perform(
            async move {
                let s = secrets.clone();
                let _ = blocking(move || s.delete(KEY_OAUTH)).await;
                let _ = blocking(move || secrets.delete(KEY_LIBRESPOT)).await;
                let _ = store.run(|s| s.clear()).await;
            },
            |()| Message::Done(Done::Noop),
        )
    }

    fn fail(&mut self, e: &Error) -> Task<Message> {
        tracing::warn!(error = %e, "request failed");
        if e.is_revoked() {
            let t = self.logout();
            self.auth = AuthState::SignedOut {
                error: Some(e.user_message()),
            };
            return t;
        }
        self.show_toast(e.user_message());
        Task::none()
    }

    fn show_toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    // ------------------------------------------------------------- loading

    fn source(&self) -> Option<DataSource> {
        self.source.clone()
    }

    fn fetch_library(&self) -> Task<Message> {
        let Some(src) = self.source() else {
            return Task::none();
        };
        Task::perform(async move { src.library().await }, |r| {
            Message::Done(Done::Library(r))
        })
    }

    fn fetch_home(&self) -> Task<Message> {
        let Some(src) = self.source() else {
            return Task::none();
        };
        Task::perform(async move { src.home().await }, |r| {
            Message::Done(Done::Home(r))
        })
    }

    fn fetch_liked_flags(&self, tracks: &[Track]) -> Task<Message> {
        let Some(src) = self.source() else {
            return Task::none();
        };
        let ids: Vec<String> = tracks
            .iter()
            .filter(|t| !t.id.is_empty())
            .take(50)
            .map(|t| t.id.clone())
            .collect();
        if ids.is_empty() {
            return Task::none();
        }
        let ids2 = ids.clone();
        Task::perform(async move { src.contains_saved(ids2).await }, move |r| {
            Message::Done(Done::LikedFlags(ids.clone(), r))
        })
    }

    /// Load the data behind the current route: cache first, then network.
    fn load_route(&mut self) -> Task<Message> {
        let route = self.route().clone();
        let Some(src) = self.source() else {
            return Task::none();
        };
        let store = &self.store;
        let task = match &route {
            Route::Home => {
                if self.home.is_some() {
                    return Task::none();
                }
                self.fetch_home()
            }
            Route::Search | Route::Library => return Task::none(),
            Route::Liked => {
                if self.liked.is_some() {
                    return Task::none();
                }
                Task::batch([
                    store_read(store, Kind::Liked, "me".into(), |p: Page<Track>| {
                        Done::Liked(Ok(p))
                    }),
                    Task::perform(async move { src.saved_tracks(0).await }, |r| {
                        Message::Done(Done::Liked(r))
                    }),
                ])
            }
            Route::Playlist(id) => {
                if self.playlists.contains_key(id) {
                    return Task::none();
                }
                let (id1, id2) = (id.clone(), id.clone());
                Task::batch([
                    store_read(store, Kind::Playlist, id.clone(), move |v| {
                        Done::Playlist(id1.clone(), Ok(v))
                    }),
                    Task::perform(async move { src.playlist(&id2).await }, {
                        let id = id.clone();
                        move |r| Message::Done(Done::Playlist(id.clone(), r))
                    }),
                ])
            }
            Route::Album(id) => {
                if self.albums.contains_key(id) {
                    return Task::none();
                }
                let (id1, id2) = (id.clone(), id.clone());
                Task::batch([
                    store_read(store, Kind::Album, id.clone(), move |v| {
                        Done::Album(id1.clone(), Ok(v))
                    }),
                    Task::perform(async move { src.album(&id2).await }, {
                        let id = id.clone();
                        move |r| Message::Done(Done::Album(id.clone(), r))
                    }),
                ])
            }
            Route::Artist(id) => {
                if self.artists.contains_key(id) {
                    return Task::none();
                }
                let (id1, id2) = (id.clone(), id.clone());
                Task::batch([
                    store_read(store, Kind::Artist, id.clone(), move |v| {
                        Done::Artist(id1.clone(), Ok(v))
                    }),
                    Task::perform(async move { src.artist_page(&id2).await }, {
                        let id = id.clone();
                        move |r| Message::Done(Done::Artist(id.clone(), r))
                    }),
                ])
            }
        };
        self.loading.insert(route.clone());
        self.errors.remove(&route);
        task
    }

    fn navigate(&mut self, route: Route) -> Task<Message> {
        self.hovered_row = None;
        self.selected_row = None;
        if !self.nav.push(route) {
            return Task::none();
        }
        self.after_nav()
    }

    fn after_nav(&mut self) -> Task<Message> {
        let key = self.route().key();
        let persist = if self.is_mock() {
            Task::none()
        } else {
            let store = self.store.clone();
            Task::perform(
                async move {
                    let _ = store.run(move |s| s.set_setting("route", &key)).await;
                },
                |()| Message::Done(Done::Noop),
            )
        };
        Task::batch([self.load_route(), persist])
    }

    fn load_more(&mut self) -> Task<Message> {
        if self.loading_more {
            return Task::none();
        }
        let Some(src) = self.source() else {
            return Task::none();
        };
        match self.route().clone() {
            Route::Liked => {
                let Some(list) = &self.liked else {
                    return Task::none();
                };
                if !list.has_more() {
                    return Task::none();
                }
                let Some(offset) = list.next else {
                    return Task::none();
                };
                self.loading_more = true;
                Task::perform(async move { src.saved_tracks(offset).await }, |r| {
                    Message::Done(Done::Liked(r))
                })
            }
            Route::Playlist(id) => {
                let Some((_, list)) = self.playlists.get(&id) else {
                    return Task::none();
                };
                if !list.has_more() {
                    return Task::none();
                }
                let Some(offset) = list.next else {
                    return Task::none();
                };
                self.loading_more = true;
                let id2 = id.clone();
                Task::perform(
                    async move { src.playlist_tracks(&id2, offset).await },
                    move |r| Message::Done(Done::PlaylistMore(id.clone(), r)),
                )
            }
            _ => Task::none(),
        }
    }

    // --------------------------------------------------------------- images

    /// Image URLs the current screen wants, with their target size.
    fn wanted_images(&self) -> Vec<(String, u32)> {
        let mut out: Vec<(String, u32)> = Vec::new();
        let mut push = |imgs: &[crate::net::model::Image], px: u32| {
            if let Some(i) = pick_image(imgs, px) {
                out.push((i.url.clone(), px));
            }
        };
        // Sidebar
        for p in &self.library.playlists {
            push(&p.images, 64);
        }
        for a in &self.library.albums {
            push(&a.images, 64);
        }
        for a in &self.library.artists {
            push(&a.images, 64);
        }
        if let Some(t) = &self.queue.current {
            push(t.images(), 64);
        }
        if self.show_queue {
            for t in self.queue.user_queue.iter().take(20) {
                push(t.images(), 64);
            }
            for t in self.queue.upcoming(20) {
                push(t.images(), 64);
            }
        }
        match self.route() {
            Route::Home => {
                if let Some(h) = &self.home {
                    for t in h.recent_tracks.iter().chain(&h.top_tracks) {
                        push(t.images(), 300);
                    }
                    for a in &h.top_artists {
                        push(&a.images, 300);
                    }
                }
                for p in self.library.playlists.iter().take(8) {
                    push(&p.images, 64);
                }
            }
            Route::Library => {
                for p in &self.library.playlists {
                    push(&p.images, 300);
                }
                for a in &self.library.albums {
                    push(&a.images, 300);
                }
                for a in &self.library.artists {
                    push(&a.images, 300);
                }
            }
            Route::Search => {
                if let Some(r) = &self.search.results {
                    for t in &r.tracks {
                        push(t.images(), 64);
                    }
                    if let Some(t) = r.tracks.first() {
                        push(t.images(), 300);
                    }
                    for a in &r.artists {
                        push(&a.images, 300);
                    }
                    for a in &r.albums {
                        push(&a.images, 300);
                    }
                    for p in &r.playlists {
                        push(&p.images, 300);
                    }
                }
            }
            Route::Liked => {
                if let Some(l) = &self.liked {
                    for t in l.tracks.iter().take(60) {
                        push(t.images(), 64);
                    }
                }
            }
            Route::Playlist(id) => {
                if let Some((p, l)) = self.playlists.get(id) {
                    push(&p.images, 300);
                    for t in l.tracks.iter().take(60) {
                        push(t.images(), 64);
                    }
                }
            }
            Route::Album(id) => {
                if let Some(a) = self.albums.get(id) {
                    push(&a.images, 300);
                }
            }
            Route::Artist(id) => {
                if let Some(a) = self.artists.get(id) {
                    push(&a.artist.images, 300);
                    for t in &a.top_tracks {
                        push(t.images(), 64);
                    }
                    for al in &a.albums {
                        push(&al.images, 300);
                    }
                }
            }
        }
        out
    }

    fn request_images(&mut self) -> Task<Message> {
        let Some(src) = self.source() else {
            return Task::none();
        };
        let mut tasks = Vec::new();
        for (url, px) in self.wanted_images() {
            if self.images.touch(&url) || self.image_requests.contains(&url) {
                continue;
            }
            if tasks.len() >= MAX_IMAGE_REQUESTS {
                break;
            }
            self.image_requests.insert(url.clone());
            let src = src.clone();
            let u = url.clone();
            tasks.push(Task::perform(
                async move { src.image(u, px).await },
                move |r| Message::Done(Done::Image(url.clone(), r)),
            ));
        }
        Task::batch(tasks)
    }

    /// Average colour of an entity's art (for gradient headers).
    pub fn art_color(&self, images: &[crate::net::model::Image]) -> Option<[u8; 3]> {
        let url = &pick_image(images, 300)?.url;
        self.images.peek(url).map(|c| c.average)
    }

    pub fn image(
        &self,
        images: &[crate::net::model::Image],
        px: u32,
    ) -> Option<iced::widget::image::Handle> {
        let url = &pick_image(images, px)?.url;
        self.images.peek(url).map(|c| c.handle.clone())
    }

    // ------------------------------------------------------------- playback

    /// Tracks behind a source, plus the context label.
    pub fn tracks_for(&self, src: &TrackSource) -> Option<(PlayContext, Vec<Track>)> {
        match src {
            TrackSource::Playlist(id) => self.playlists.get(id).map(|(p, l)| {
                (
                    PlayContext {
                        name: p.name.clone(),
                        route: Some(Route::Playlist(id.clone())),
                    },
                    l.tracks.clone(),
                )
            }),
            TrackSource::Album(id) => self.albums.get(id).map(|a| {
                (
                    PlayContext {
                        name: a.name.clone(),
                        route: Some(Route::Album(id.clone())),
                    },
                    a.tracks.clone(),
                )
            }),
            TrackSource::Liked => self.liked.as_ref().map(|l| {
                (
                    PlayContext {
                        name: "Liked Songs".into(),
                        route: Some(Route::Liked),
                    },
                    l.tracks.clone(),
                )
            }),
            TrackSource::ArtistTop(id) => self.artists.get(id).map(|a| {
                (
                    PlayContext {
                        name: a.artist.name.clone(),
                        route: Some(Route::Artist(id.clone())),
                    },
                    a.top_tracks.clone(),
                )
            }),
            TrackSource::Search => self.search.results.as_ref().map(|r| {
                (
                    PlayContext {
                        name: format!("Search: {}", r.query),
                        route: Some(Route::Search),
                    },
                    r.tracks.clone(),
                )
            }),
            TrackSource::HomeRecent => self.home.as_ref().map(|h| {
                (
                    PlayContext {
                        name: "Recently played".into(),
                        route: Some(Route::Home),
                    },
                    h.recent_tracks.clone(),
                )
            }),
            TrackSource::HomeTop => self.home.as_ref().map(|h| {
                (
                    PlayContext {
                        name: "Your top tracks".into(),
                        route: Some(Route::Home),
                    },
                    h.top_tracks.clone(),
                )
            }),
        }
    }

    /// Is `src` the context currently playing?
    pub fn is_playing_source(&self, src: &TrackSource) -> bool {
        let Some(ctx) = &self.queue.context else {
            return false;
        };
        let route = match src {
            TrackSource::Playlist(id) => Route::Playlist(id.clone()),
            TrackSource::Album(id) => Route::Album(id.clone()),
            TrackSource::Liked => Route::Liked,
            TrackSource::ArtistTop(id) => Route::Artist(id.clone()),
            _ => return false,
        };
        ctx.route.as_ref() == Some(&route) && self.playback.is_playing()
    }

    fn engine_send(&mut self, cmd: PlaybackCommand) {
        match &self.engine {
            Some(e) => e.send(cmd),
            None => tracing::debug!("engine not ready yet"),
        }
    }

    fn start_track(&mut self, track: Track, position_ms: u32) {
        self.reload_on_play = false;
        self.playback.duration_ms = track.duration_ms;
        self.playback
            .report(PlayStatus::Loading, position_ms, self.now);
        let cmd = PlaybackCommand::Load {
            uri: track.uri.clone(),
            duration_ms: track.duration_ms,
            start: true,
            position_ms,
        };
        if !self.is_mock() && self.session.is_none() {
            self.show_toast("Connecting to Spotify audio…");
        }
        self.engine_send(cmd);
    }

    /// Resume after pause; reloads the track if the engine lost it.
    fn resume(&mut self) {
        if self.reload_on_play
            && let Some(t) = self.queue.current.clone()
        {
            let pos = self.playback.position(self.now);
            self.start_track(t, pos);
        } else {
            self.engine_send(PlaybackCommand::Play);
        }
    }

    fn pause_for_reconnect(&mut self) {
        let pos = self.playback.position(self.now);
        self.playback.report(PlayStatus::Paused, pos, self.now);
        self.reload_on_play = true;
    }

    /// The engine reported a dead audio session: keep the queue, pause, and
    /// reconnect with backoff.
    fn on_session_lost(&mut self) -> Task<Message> {
        if self.is_mock() {
            return Task::none();
        }
        let had_session = self.session.take().is_some();
        if self.queue.current.is_some() {
            self.pause_for_reconnect();
        }
        if !had_session || self.source.is_none() {
            // A reconnect is already pending (or we are logged out).
            return Task::none();
        }
        self.show_toast("Connection to Spotify lost. Reconnecting…");
        self.schedule_audio_reconnect(&Error::Net("audio session lost".into()))
    }

    fn schedule_audio_reconnect(&mut self, e: &Error) -> Task<Message> {
        tracing::warn!(error = %e, attempt = self.audio_attempt, "audio session failed");
        self.connection = Connection::Reconnecting {
            attempt: self.audio_attempt,
        };
        let delay = backoff_delay(self.audio_attempt, rand::random());
        self.audio_attempt = self.audio_attempt.saturating_add(1);
        if self.audio_attempt == 3 {
            self.show_toast(e.user_message());
        }
        Task::perform(async move { tokio::time::sleep(delay).await }, |()| {
            Message::Done(Done::ReconnectAudio)
        })
    }

    fn stop_playback(&mut self) {
        self.engine_send(PlaybackCommand::Stop);
        self.playback.report(PlayStatus::Stopped, 0, self.now);
    }

    fn play_from(&mut self, src: TrackSource, index: Option<usize>) -> Task<Message> {
        // Clicking the big play button of the context that is playing pauses it.
        if index.is_none() && self.is_playing_source(&src) {
            self.engine_send(PlaybackCommand::Pause);
            return Task::none();
        }
        if index.is_none()
            && let Some(ctx) = &self.queue.context
            && self.playback.status == PlayStatus::Paused
            && self
                .tracks_for(&src)
                .is_some_and(|(c, _)| c.route == ctx.route)
        {
            self.engine_send(PlaybackCommand::Play);
            return Task::none();
        }
        let Some((ctx, tracks)) = self.tracks_for(&src) else {
            return Task::none();
        };
        match self.queue.play_context(ctx, tracks, index) {
            Some(t) => self.start_track(t, 0),
            None => self.show_toast("Nothing playable here."),
        }
        Task::none()
    }

    fn advance(&mut self, auto: bool) {
        match self.queue.next(auto) {
            Some(t) => self.start_track(t, 0),
            None => self.stop_playback(),
        }
    }

    fn persist_setting(&self, key: &'static str, value: String) -> Task<Message> {
        let store = self.store.clone();
        Task::perform(
            async move {
                let _ = store.run(move |s| s.set_setting(key, &value)).await;
            },
            |()| Message::Done(Done::Noop),
        )
    }

    fn current_uri(&self) -> Option<&str> {
        self.queue.current.as_ref().map(|t| t.uri.as_str())
    }

    fn on_playback(&mut self, e: PlaybackEvent) -> Task<Message> {
        match e {
            PlaybackEvent::Ready(h) => {
                h.send(PlaybackCommand::SetVolume(self.playback.effective_volume()));
                if let Some(s) = &self.session {
                    h.send(PlaybackCommand::Attach(s.clone()));
                }
                self.engine = Some(h);
                if let Some(plan) = &self.screenshot
                    && plan.screen != "login"
                    && let Some(t) = self.queue.current.clone()
                {
                    // Screenshots show a track mid-way through.
                    self.start_track(t, 72_000);
                }
            }
            PlaybackEvent::Loading { uri } if self.current_uri() == Some(&uri) => {
                self.playback.status = PlayStatus::Loading;
            }
            PlaybackEvent::Playing { uri, position_ms } if self.current_uri() == Some(&uri) => {
                self.playback
                    .report(PlayStatus::Playing, position_ms, self.now);
            }
            PlaybackEvent::Paused { uri, position_ms } if self.current_uri() == Some(&uri) => {
                self.playback
                    .report(PlayStatus::Paused, position_ms, self.now);
            }
            PlaybackEvent::Position { uri, position_ms } if self.current_uri() == Some(&uri) => {
                let status = self.playback.status;
                self.playback.report(status, position_ms, self.now);
            }
            PlaybackEvent::EndOfTrack { uri } if self.current_uri() == Some(&uri) => {
                self.advance(true);
            }
            PlaybackEvent::Unavailable { uri } if self.current_uri() == Some(&uri) => {
                if !self.is_mock() && self.session.is_none() {
                    // No audio session: every track would be "unavailable".
                    // Pause instead of skipping through the whole queue.
                    self.pause_for_reconnect();
                } else {
                    self.show_toast("This song is not available. Skipping.");
                    self.advance(false);
                }
            }
            PlaybackEvent::SessionLost => return self.on_session_lost(),
            PlaybackEvent::Stopped => {
                if self.playback.status != PlayStatus::Loading {
                    self.playback.status = PlayStatus::Stopped;
                }
            }
            PlaybackEvent::Error(m) => {
                tracing::warn!(error = %m, "playback");
                self.show_toast(Error::Playback(m).user_message());
            }
            _ => {}
        }
        Task::none()
    }

    fn on_media(&mut self, e: MediaEvent) -> Task<Message> {
        match e {
            MediaEvent::Ready(h) => {
                self.media = Some(h);
                self.last_media = None;
                Task::none()
            }
            MediaEvent::Unavailable(reason) => {
                tracing::info!(%reason, "MPRIS disabled");
                self.mpris_enabled = false;
                Task::none()
            }
            MediaEvent::Command(c) => match c {
                MediaCommand::Play => {
                    if self.playback.status == PlayStatus::Paused {
                        self.resume();
                    } else if self.playback.status == PlayStatus::Stopped
                        && let Some(t) = self.queue.current.clone()
                    {
                        self.start_track(t, 0);
                    }
                    Task::none()
                }
                MediaCommand::Pause | MediaCommand::Stop => {
                    self.engine_send(PlaybackCommand::Pause);
                    Task::none()
                }
                MediaCommand::PlayPause => self.update_ui(Ui::TogglePlay),
                MediaCommand::Next => self.update_ui(Ui::Next),
                MediaCommand::Previous => self.update_ui(Ui::Previous),
                MediaCommand::Seek(offset_us) => {
                    let pos = i64::from(self.playback.position(self.now)) + offset_us / 1000;
                    self.seek_to(pos.max(0) as u32);
                    Task::none()
                }
                MediaCommand::SetPosition {
                    track_id,
                    position_us,
                } => {
                    if self
                        .queue
                        .current
                        .as_ref()
                        .is_some_and(|t| crate::net::mpris::track_path(&t.id).ends_with(&track_id))
                    {
                        self.seek_to((position_us / 1000).max(0) as u32);
                    }
                    Task::none()
                }
                MediaCommand::SetVolume(v) => {
                    let t = self.update_ui(Ui::Volume(v as f32));
                    Task::batch([t, self.update_ui(Ui::VolumeCommit)])
                }
                MediaCommand::SetShuffle(on) => {
                    if on != self.queue.shuffle {
                        return self.update_ui(Ui::ToggleShuffle);
                    }
                    Task::none()
                }
                MediaCommand::SetLoop(l) => {
                    self.queue.repeat = match l {
                        MediaLoop::None => Repeat::Off,
                        MediaLoop::Playlist => Repeat::Context,
                        MediaLoop::Track => Repeat::Track,
                    };
                    self.persist_setting("repeat", self.queue.repeat.as_str().into())
                }
                MediaCommand::Raise => iced::window::latest().and_then(iced::window::gain_focus),
                MediaCommand::Quit => iced::exit(),
            },
        }
    }

    /// Build the MPRIS snapshot and push it if anything changed.
    fn push_media_snapshot(&mut self) {
        let Some(media) = &self.media else {
            return;
        };
        let snap = self.media_snapshot();
        if self.last_media.as_ref() != Some(&snap) {
            media.update(snap.clone());
            self.last_media = Some(snap);
        }
    }

    pub fn media_snapshot(&self) -> MediaSnapshot {
        let track = self.queue.current.as_ref().map(|t| MediaTrack {
            id: t.id.clone(),
            title: t.name.clone(),
            artists: t.artists.iter().map(|a| a.name.clone()).collect(),
            album: t.album.as_ref().map(|a| a.name.clone()).unwrap_or_default(),
            length_ms: t.duration_ms,
            art_url: pick_image(t.images(), 300)
                .map(|i| i.url.clone())
                .filter(|u| u.starts_with("http")),
        });
        MediaSnapshot {
            status: match self.playback.status {
                PlayStatus::Playing | PlayStatus::Loading => MediaStatus::Playing,
                PlayStatus::Paused => MediaStatus::Paused,
                PlayStatus::Stopped => MediaStatus::Stopped,
            },
            can_next: track.is_some() && self.queue.has_next(),
            can_previous: track.is_some(),
            track,
            position_ms: self.playback.position(self.now),
            volume: f64::from(self.playback.effective_volume()),
            shuffle: self.queue.shuffle,
            loop_status: match self.queue.repeat {
                Repeat::Off => MediaLoop::None,
                Repeat::Context => MediaLoop::Playlist,
                Repeat::Track => MediaLoop::Track,
            },
        }
    }

    fn seek_to(&mut self, ms: u32) {
        let ms = ms.min(self.playback.duration_ms);
        self.engine_send(PlaybackCommand::Seek(ms));
        let status = self.playback.status;
        self.playback.report(status, ms, self.now);
    }

    // ------------------------------------------------------------------- ui

    fn update_ui(&mut self, u: Ui) -> Task<Message> {
        match u {
            Ui::Navigate(r) => self.navigate(r),
            Ui::Back => {
                if self.nav.back() {
                    self.after_nav()
                } else {
                    Task::none()
                }
            }
            Ui::Forward => {
                if self.nav.forward() {
                    self.after_nav()
                } else {
                    Task::none()
                }
            }
            Ui::SearchInput(q) => {
                self.search.query = q;
                self.search.seq += 1;
                let nav = self.navigate(Route::Search);
                if self.search.query.trim().is_empty() {
                    self.search.results = None;
                    self.search.loading = false;
                    return nav;
                }
                let seq = self.search.seq;
                Task::batch([
                    nav,
                    Task::perform(
                        async move { tokio::time::sleep(Duration::from_millis(250)).await },
                        move |()| Message::Done(Done::SearchDebounced(seq)),
                    ),
                ])
            }
            Ui::SearchSubmit => self.on_done(Done::SearchDebounced(self.search.seq)),
            Ui::SearchFilter(f) => {
                self.search.filter = f;
                Task::none()
            }
            Ui::LibraryFilter(f) => {
                self.library_filter = if self.library_filter == f {
                    LibraryFilter::All
                } else {
                    f
                };
                Task::none()
            }
            Ui::PlayFrom(src, index) => self.play_from(src, index),
            Ui::TogglePlay => {
                match self.playback.status {
                    PlayStatus::Playing | PlayStatus::Loading => {
                        self.engine_send(PlaybackCommand::Pause)
                    }
                    PlayStatus::Paused => self.resume(),
                    PlayStatus::Stopped => {
                        if let Some(t) = self.queue.current.clone() {
                            self.start_track(t, 0);
                        }
                    }
                }
                Task::none()
            }
            Ui::Next => {
                if self.queue.current.is_some() {
                    self.advance(false);
                }
                Task::none()
            }
            Ui::Previous => {
                match self.queue.previous(self.playback.position(self.now)) {
                    Previous::Restart => {
                        if self.queue.current.is_some() {
                            self.seek_to(0);
                        }
                    }
                    Previous::Play(t) => self.start_track(t, 0),
                }
                Task::none()
            }
            Ui::ToggleShuffle => {
                let on = !self.queue.shuffle;
                self.queue.set_shuffle(on);
                self.persist_setting("shuffle", if on { "1" } else { "0" }.into())
            }
            Ui::CycleRepeat => {
                let r = self.queue.cycle_repeat();
                self.persist_setting("repeat", r.as_str().into())
            }
            Ui::SeekPreview(f) => {
                if self.playback.duration_ms > 0 {
                    self.playback.seek_preview =
                        Some((f.clamp(0.0, 1.0) * self.playback.duration_ms as f32) as u32);
                }
                Task::none()
            }
            Ui::SeekCommit => {
                if let Some(p) = self.playback.seek_preview.take() {
                    self.seek_to(p);
                }
                Task::none()
            }
            Ui::Volume(v) => {
                self.playback.volume = v.clamp(0.0, 1.0);
                self.playback.muted = false;
                self.engine_send(PlaybackCommand::SetVolume(self.playback.volume));
                Task::none()
            }
            Ui::VolumeCommit => {
                self.persist_setting("volume", format!("{:.3}", self.playback.volume))
            }
            Ui::ToggleMute => {
                self.playback.muted = !self.playback.muted;
                self.engine_send(PlaybackCommand::SetVolume(self.playback.effective_volume()));
                Task::none()
            }
            Ui::ToggleQueue => {
                self.show_queue = !self.show_queue;
                Task::none()
            }
            Ui::PlayUpcoming(k) => {
                if let Some(t) = self.queue.jump_upcoming(k) {
                    self.start_track(t, 0);
                }
                Task::none()
            }
            Ui::PlayQueued(k) => {
                if let Some(t) = self.queue.jump_user_queue(k) {
                    self.start_track(t, 0);
                }
                Task::none()
            }
            Ui::Enqueue(t) => {
                self.queue.enqueue(t);
                self.show_toast("Added to queue");
                Task::none()
            }
            Ui::ToggleLike(t) => {
                let Some(src) = self.source() else {
                    return Task::none();
                };
                let saved = !self.liked_ids.contains(&t.id);
                if saved {
                    self.liked_ids.insert(t.id.clone());
                } else {
                    self.liked_ids.remove(&t.id);
                }
                self.show_toast(if saved {
                    "Added to Liked Songs"
                } else {
                    "Removed from Liked Songs"
                });
                let id = t.id.clone();
                Task::perform(
                    async move { src.set_saved(vec![t.id], saved).await },
                    move |r| Message::Done(Done::LikeSaved(id.clone(), saved, r)),
                )
            }
            Ui::HoverRow(k) => {
                self.hovered_row = k;
                Task::none()
            }
            Ui::SelectRow(k) => {
                self.selected_row = Some(k);
                Task::none()
            }
            Ui::Login => self.start_login(),
            Ui::CancelLogin => {
                if let Some(h) = self.login_abort.take() {
                    h.abort();
                }
                self.auth = AuthState::SignedOut { error: None };
                Task::none()
            }
            Ui::OpenLoginUrl => {
                if let AuthState::WaitingForBrowser { url } = &self.auth
                    && let Err(e) = oauth::open_browser(url)
                {
                    self.show_toast(e.user_message());
                }
                Task::none()
            }
            Ui::CopyLoginUrl => {
                if let AuthState::WaitingForBrowser { url } = &self.auth {
                    let url = url.clone();
                    self.show_toast("Login link copied");
                    return iced::clipboard::write(url);
                }
                Task::none()
            }
            Ui::Logout => self.logout(),
            Ui::Scrolled(rel_y) => {
                if rel_y > 0.9 {
                    self.load_more()
                } else {
                    Task::none()
                }
            }
            Ui::DismissToast => {
                self.toast = None;
                Task::none()
            }
            Ui::Tick(now) => {
                self.now = now;
                if self.toast.as_ref().is_some_and(|(_, at)| {
                    now.saturating_duration_since(*at) > Duration::from_secs(TOAST_SECS)
                }) {
                    self.toast = None;
                }
                Task::none()
            }
        }
    }

    // ----------------------------------------------------------------- done

    fn on_done(&mut self, d: Done) -> Task<Message> {
        match d {
            Done::Restored(Ok(Some(ready))) => {
                self.auth = self.auth.apply(AuthEvent::StoredSessionFound);
                self.signed_in(ready)
            }
            Done::Restored(Ok(None)) => {
                self.auth = self.auth.apply(AuthEvent::NoStoredSession);
                Task::none()
            }
            Done::Restored(Err(e)) | Done::LoggedIn(Err(e)) => {
                self.login_abort = None;
                self.auth = self.auth.apply(AuthEvent::Failed(e));
                Task::none()
            }
            Done::LoggedIn(Ok(ready)) => {
                self.login_abort = None;
                self.auth = self.auth.apply(AuthEvent::CallbackReceived);
                self.signed_in(ready)
            }
            Done::Profile(Ok(user)) => {
                if !user.is_premium() && !self.is_mock() {
                    self.show_toast("Spotify Premium is required for playback.");
                }
                self.auth = AuthState::SignedIn { user: user.clone() };
                self.user = Some(user.clone());
                store_write(&self.store, Kind::User, "me".into(), user)
            }
            Done::Profile(Err(e)) => self.fail(&e),
            Done::Audio(Ok(session)) => {
                self.audio_attempt = 0;
                self.connection = Connection::Connected;
                if let Some(h) = &self.engine {
                    h.send(PlaybackCommand::Attach(session.clone()));
                }
                self.session = Some(session);
                Task::none()
            }
            Done::Audio(Err(e)) => {
                if e.is_revoked() || self.source.is_none() {
                    return self.fail(&e);
                }
                self.schedule_audio_reconnect(&e)
            }
            Done::ReconnectAudio => {
                if self.source.is_some() && self.session.is_none() {
                    self.connection = Connection::Connecting;
                    self.connect_audio_task()
                } else {
                    Task::none()
                }
            }
            Done::Library(Ok(lib)) => {
                self.library = lib.clone();
                if self.is_mock() {
                    Task::none()
                } else {
                    store_write(&self.store, Kind::Library, "me".into(), lib)
                }
            }
            Done::Home(Ok(home)) => {
                self.loading.remove(&Route::Home);
                let flags = self.fetch_liked_flags(&home.recent_tracks);
                self.home = Some(home.clone());
                Task::batch([
                    flags,
                    store_write(&self.store, Kind::Home, "me".into(), home),
                ])
            }
            Done::Library(Err(e)) => self.fail(&e),
            Done::Home(Err(e)) => {
                self.loading.remove(&Route::Home);
                self.errors.insert(Route::Home, e.user_message());
                self.fail(&e)
            }
            Done::Playlist(id, Ok((pl, page))) => {
                let route = Route::Playlist(id.clone());
                self.loading.remove(&route);
                let flags = self.fetch_liked_flags(&page.items);
                let list = TrackList::from_page(page.clone());
                self.playlists.insert(id.clone(), (pl.clone(), list));
                Task::batch([
                    flags,
                    store_write(&self.store, Kind::Playlist, id, (pl, page)),
                ])
            }
            Done::PlaylistMore(id, Ok(page)) => {
                self.loading_more = false;
                if let Some((_, list)) = self.playlists.get_mut(&id) {
                    list.append(page);
                }
                Task::none()
            }
            Done::Liked(Ok(page)) => {
                self.loading.remove(&Route::Liked);
                self.loading_more = false;
                for t in &page.items {
                    self.liked_ids.insert(t.id.clone());
                }
                let first = page.offset == 0;
                if first {
                    self.liked = Some(TrackList::from_page(page.clone()));
                } else if let Some(list) = self.liked.as_mut() {
                    list.append(page.clone());
                }
                if first {
                    store_write(&self.store, Kind::Liked, "me".into(), page)
                } else {
                    Task::none()
                }
            }
            Done::Album(id, Ok(album)) => {
                self.loading.remove(&Route::Album(id.clone()));
                let flags = self.fetch_liked_flags(&album.tracks);
                self.albums.insert(id.clone(), album.clone());
                Task::batch([flags, store_write(&self.store, Kind::Album, id, album)])
            }
            Done::Artist(id, Ok(page)) => {
                self.loading.remove(&Route::Artist(id.clone()));
                let flags = self.fetch_liked_flags(&page.top_tracks);
                self.artists.insert(id.clone(), page.clone());
                Task::batch([flags, store_write(&self.store, Kind::Artist, id, page)])
            }
            Done::Playlist(id, Err(e)) => self.route_failed(Route::Playlist(id), &e),
            Done::Album(id, Err(e)) => self.route_failed(Route::Album(id), &e),
            Done::Artist(id, Err(e)) => self.route_failed(Route::Artist(id), &e),
            Done::Liked(Err(e)) => self.route_failed(Route::Liked, &e),
            Done::PlaylistMore(_, Err(e)) => {
                self.loading_more = false;
                self.fail(&e)
            }
            Done::SearchDebounced(seq) => {
                if seq != self.search.seq || self.search.query.trim().is_empty() {
                    return Task::none();
                }
                let Some(src) = self.source() else {
                    return Task::none();
                };
                self.search.loading = true;
                let q = self.search.query.trim().to_owned();
                Task::perform(async move { src.search(&q).await }, move |r| {
                    Message::Done(Done::Search(seq, r))
                })
            }
            Done::Search(seq, r) => {
                if seq != self.search.seq {
                    return Task::none();
                }
                self.search.loading = false;
                match r {
                    Ok(res) => {
                        self.search.error = None;
                        let flags = self.fetch_liked_flags(&res.tracks);
                        self.search.results = Some(res);
                        flags
                    }
                    Err(e) => {
                        self.search.error = Some(e.user_message());
                        self.fail(&e)
                    }
                }
            }
            Done::Image(url, r) => {
                self.image_requests.remove(&url);
                match r {
                    Ok(img) => self.images.insert(url, img),
                    Err(e) => tracing::debug!(error = %e, "image fetch failed"),
                }
                Task::none()
            }
            Done::LikeSaved(id, saved, r) => {
                if let Err(e) = r {
                    // Roll back the optimistic update.
                    if saved {
                        self.liked_ids.remove(&id);
                    } else {
                        self.liked_ids.insert(id);
                    }
                    return self.fail(&e);
                }
                // Liked Songs changed: refetch it next time it is opened.
                self.liked = None;
                Task::none()
            }
            Done::LikedFlags(ids, Ok(flags)) => {
                for (id, liked) in ids.into_iter().zip(flags) {
                    if liked {
                        self.liked_ids.insert(id);
                    } else {
                        self.liked_ids.remove(&id);
                    }
                }
                Task::none()
            }
            Done::LikedFlags(_, Err(e)) => {
                tracing::debug!(error = %e, "liked flags unavailable");
                Task::none()
            }
            Done::Screenshot(shot) => {
                let Some(plan) = self.screenshot.clone() else {
                    return Task::none();
                };
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || save_png(&plan.out, &shot))
                            .await
                            .map_err(|_| Error::Cancelled)?
                    },
                    |r| Message::Done(Done::ScreenshotSaved(r)),
                )
            }
            Done::ScreenshotSaved(r) => {
                match r {
                    Ok(()) => eprintln!("screenshot saved"),
                    Err(e) => eprintln!("screenshot failed: {e}"),
                }
                iced::exit()
            }
            Done::Quit => iced::exit(),
            Done::Noop => Task::none(),
        }
    }

    fn route_failed(&mut self, route: Route, e: &Error) -> Task<Message> {
        self.loading.remove(&route);
        self.errors.insert(route, e.user_message());
        if e.is_revoked() {
            self.fail(e)
        } else {
            Task::none()
        }
    }

    // ------------------------------------------------------------ screenshot

    fn prepare_screenshot(&mut self, plan: &ScreenshotPlan) -> Task<Message> {
        // Pre-populate a playing queue so the now-playing bar has content.
        if plan.screen != "login"
            && let Ok((pl, page)) = crate::mock::playlist("pl01")
        {
            self.queue.play_context(
                PlayContext {
                    name: pl.name.clone(),
                    route: Some(Route::Playlist(pl.id.clone())),
                },
                page.items.clone(),
                Some(2),
            );
            self.playback.duration_ms = self.queue.current.as_ref().map_or(0, |t| t.duration_ms);
            self.queue.enqueue(page.items[7].clone());
            self.queue.enqueue(page.items[9].clone());
            if let Some(t) = &self.queue.current {
                self.liked_ids.insert(t.id.clone());
            }
        }
        let mut tasks = Vec::new();
        match plan.screen.as_str() {
            "login" => {
                self.auth = AuthState::SignedOut { error: None };
                self.source = None;
            }
            "search" => {
                self.nav = Nav::new(Route::Search);
                self.search.query = "nova".into();
                self.search.seq = 1;
                tasks.push(self.on_done(Done::SearchDebounced(1)));
            }
            "library" => {
                self.nav = Nav::new(Route::Library);
            }
            "playlist" => {
                self.nav = Nav::new(Route::Home);
                tasks.push(self.navigate(Route::Playlist("pl01".into())));
            }
            "album" => {
                self.nav = Nav::new(Route::Home);
                tasks.push(self.navigate(Route::Album("al03".into())));
            }
            "artist" => {
                self.nav = Nav::new(Route::Home);
                tasks.push(self.navigate(Route::Artist("ar03".into())));
            }
            "liked" => {
                self.nav = Nav::new(Route::Home);
                tasks.push(self.navigate(Route::Liked));
            }
            "queue" => {
                self.show_queue = true;
                self.nav = Nav::new(Route::Home);
                tasks.push(self.navigate(Route::Album("al01".into())));
            }
            _ => {
                self.nav = Nav::new(Route::Home);
            }
        }
        tasks.push(
            Task::perform(
                async move { tokio::time::sleep(Duration::from_millis(2500)).await },
                |()| (),
            )
            .then(|()| iced::window::latest())
            .then(|id| match id {
                Some(id) => iced::window::screenshot(id),
                None => Task::none(),
            })
            .map(|shot| Message::Done(Done::Screenshot(shot))),
        );
        Task::batch(tasks)
    }
}

fn save_png(path: &std::path::Path, shot: &iced::window::Screenshot) -> Result<()> {
    let (w, h) = (shot.size.width, shot.size.height);
    let img = image::RgbaImage::from_raw(w, h, shot.rgba.to_vec())
        .ok_or_else(|| Error::Io("screenshot buffer has the wrong size".into()))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    img.save_with_format(path, image::ImageFormat::Png)
        .map_err(|e| Error::Io(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_app() -> App {
        let (app, _) = App::boot(Config::for_tests(), Options::default());
        app
    }

    fn feed(app: &mut App, d: Done) {
        let _ = app.update(Message::Done(d));
    }

    /// A real (non-mock) app with a fake Web source and a fake engine.
    fn real_app_playing() -> (
        App,
        tokio::sync::mpsc::UnboundedReceiver<PlaybackCommand>,
        Vec<Track>,
    ) {
        let mut cfg = Config::for_tests();
        cfg.mock = false;
        let (mut app, _) = App::boot(cfg, Options::default());
        app.secrets = Arc::new(MemoryStore::new());
        app.store = Store::in_memory().unwrap();
        app.source = Some(DataSource::Mock);
        app.auth = AuthState::SignedIn {
            user: crate::mock::user(),
        };
        let (h, rx) = PlaybackHandle::channel();
        app.engine = Some(h);
        let (pl, page) = crate::mock::playlist("pl01").unwrap();
        app.playlists
            .insert("pl01".into(), (pl, TrackList::from_page(page.clone())));
        (app, rx, page.items)
    }

    #[tokio::test]
    async fn lost_session_pauses_and_reloads_instead_of_skipping() {
        let (mut app, mut rx, tracks) = real_app_playing();
        // Pretend a session was attached (librespot sessions can't be built
        // without a network, so use the "had a session" path via the flag).
        let _ = app.update(ui(Ui::PlayFrom(
            TrackSource::Playlist("pl01".into()),
            Some(0),
        )));
        let uri = tracks[0].uri.clone();
        let _ = app.update(Message::Net(Net::Playback(PlaybackEvent::Playing {
            uri: uri.clone(),
            position_ms: 30_000,
        })));
        while rx.try_recv().is_ok() {}

        // An unconnected librespot session stands in for the dead one.
        app.session = Some(SpotifySession(librespot_core::Session::new(
            librespot_core::SessionConfig::default(),
            None,
        )));
        let _ = app.update(Message::Net(Net::Playback(PlaybackEvent::SessionLost)));
        assert!(app.session.is_none());
        assert_eq!(app.connection, Connection::Reconnecting { attempt: 0 });
        assert_eq!(app.playback.status, PlayStatus::Paused);

        // Unavailable while the session is gone must not skip the queue.
        let _ = app.update(Message::Net(Net::Playback(PlaybackEvent::Unavailable {
            uri: uri.clone(),
        })));
        assert_eq!(app.queue.current.as_ref().unwrap().uri, uri);
        assert_eq!(app.playback.status, PlayStatus::Paused);

        // Pressing play reloads the track at the saved position.
        let _ = app.update(ui(Ui::TogglePlay));
        let mut reloaded = None;
        while let Ok(cmd) = rx.try_recv() {
            if let PlaybackCommand::Load {
                uri, position_ms, ..
            } = cmd
            {
                reloaded = Some((uri, position_ms));
            }
        }
        let (u, pos) = reloaded.expect("track reloaded");
        assert_eq!(u, uri);
        assert!(pos >= 30_000);
    }

    #[test]
    fn session_lost_schedules_reconnect_with_backoff() {
        let (mut app, _rx, _) = real_app_playing();
        app.reload_on_play = false;
        // Simulate an established session being reported dead twice.
        app.audio_attempt = 0;
        let _ = app.schedule_audio_reconnect(&Error::Net("x".into()));
        assert_eq!(app.connection, Connection::Reconnecting { attempt: 0 });
        let _ = app.schedule_audio_reconnect(&Error::Net("x".into()));
        assert_eq!(app.connection, Connection::Reconnecting { attempt: 1 });
        let _ = app.update(Message::Done(Done::ReconnectAudio));
        assert_eq!(
            app.connection,
            Connection::Connecting,
            "retries while no session"
        );
    }

    #[test]
    fn mock_boot_is_signed_in() {
        let app = mock_app();
        assert!(app.auth.is_signed_in());
        assert!(app.source.is_some());
        assert_eq!(app.route(), &Route::Home);
    }

    #[test]
    fn playing_a_playlist_and_advancing() {
        let mut app = mock_app();
        let (h, mut rx) = PlaybackHandle::channel();
        let _ = app.update(Message::Net(Net::Playback(PlaybackEvent::Ready(h))));
        let (pl, page) = crate::mock::playlist("pl01").unwrap();
        feed(
            &mut app,
            Done::Playlist("pl01".into(), Ok((pl, page.clone()))),
        );
        let _ = app.update(ui(Ui::PlayFrom(
            TrackSource::Playlist("pl01".into()),
            Some(1),
        )));
        assert_eq!(app.queue.current.as_ref().unwrap().id, page.items[1].id);
        // Drain: SetVolume, Load
        let mut loads = 0;
        while let Ok(cmd) = rx.try_recv() {
            if matches!(cmd, PlaybackCommand::Load { .. }) {
                loads += 1;
            }
        }
        assert_eq!(loads, 1);
        let uri = page.items[1].uri.clone();
        let _ = app.update(Message::Net(Net::Playback(PlaybackEvent::Playing {
            uri: uri.clone(),
            position_ms: 0,
        })));
        assert_eq!(app.playback.status, PlayStatus::Playing);
        // A stale event for another track is ignored.
        let _ = app.update(Message::Net(Net::Playback(PlaybackEvent::EndOfTrack {
            uri: "spotify:track:other".into(),
        })));
        assert_eq!(app.queue.current.as_ref().unwrap().id, page.items[1].id);
        let _ = app.update(Message::Net(Net::Playback(PlaybackEvent::EndOfTrack {
            uri,
        })));
        assert_eq!(app.queue.current.as_ref().unwrap().id, page.items[2].id);
        let snap = app.media_snapshot();
        assert_eq!(snap.track.unwrap().title, page.items[2].name);
    }

    #[test]
    fn navigation_and_search_debounce() {
        let mut app = mock_app();
        let _ = app.update(ui(Ui::SearchInput("nova".into())));
        assert_eq!(app.route(), &Route::Search);
        let seq = app.search.seq;
        let _ = app.update(ui(Ui::SearchInput("nova h".into())));
        // The stale debounce is ignored, the current one starts a search.
        feed(&mut app, Done::SearchDebounced(seq));
        assert!(!app.search.loading);
        feed(&mut app, Done::SearchDebounced(seq + 1));
        assert!(app.search.loading);
        feed(&mut app, Done::Search(seq, Ok(SearchResults::default())));
        assert!(app.search.loading, "stale results are dropped");
        feed(
            &mut app,
            Done::Search(seq + 1, Ok(crate::mock::search("nova"))),
        );
        assert!(!app.search.loading);
        assert!(app.search.results.is_some());
        let _ = app.update(ui(Ui::Back));
        assert_eq!(app.route(), &Route::Home);
    }

    #[test]
    fn revoked_error_logs_out() {
        let mut app = mock_app();
        feed(&mut app, Done::Library(Err(Error::revoked("gone"))));
        assert!(matches!(app.auth, AuthState::SignedOut { error: Some(_) }));
        assert!(app.source.is_none());
    }

    #[test]
    fn like_is_optimistic_and_rolls_back() {
        let mut app = mock_app();
        let t = crate::mock::album("al01").unwrap().tracks[1].clone();
        let _ = app.update(ui(Ui::ToggleLike(t.clone())));
        assert!(app.liked_ids.contains(&t.id));
        feed(
            &mut app,
            Done::LikeSaved(t.id.clone(), true, Err(Error::Net("x".into()))),
        );
        assert!(!app.liked_ids.contains(&t.id));
    }

    #[test]
    fn media_commands_map_to_actions() {
        let mut app = mock_app();
        let _ = app.update(Message::Net(Net::Media(MediaEvent::Command(
            MediaCommand::SetShuffle(true),
        ))));
        assert!(app.queue.shuffle);
        let _ = app.update(Message::Net(Net::Media(MediaEvent::Command(
            MediaCommand::SetLoop(MediaLoop::Track),
        ))));
        assert_eq!(app.queue.repeat, Repeat::Track);
        let _ = app.update(Message::Net(Net::Media(MediaEvent::Command(
            MediaCommand::SetVolume(0.25),
        ))));
        assert!((app.playback.volume - 0.25).abs() < 1e-6);
        assert_eq!(app.media_snapshot().loop_status, MediaLoop::Track);
    }
}
