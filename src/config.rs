//! Runtime configuration: XDG paths, preferences and (advanced) env overrides.
//!
//! Precedence: environment variables > `config.json` > defaults.
//! `config.json` lives at `$XDG_CONFIG_HOME/ferrotune/config.json`; it is
//! optional and nothing in it is required to log in.

use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const APP_ID: &str = "dev.ferro.ferrotune";
pub const APP_NAME: &str = "Ferrotune";
pub const DEFAULT_REDIRECT_PORT: u16 = 8898;
pub const SPOTIFY_ACCOUNTS: &str = "https://accounts.spotify.com";
/// GraphQL "pathfinder" host used by the desktop and web clients.
pub const SPOTIFY_PATHFINDER: &str = "https://api-partner.spotify.com";

/// Scopes requested at the browser login. This is the list the `librespot`
/// binary requests with the same desktop client id; the login token is only
/// used once to open the librespot session, which then issues its own tokens.
pub const SCOPES: &[&str] = &[
    "app-remote-control",
    "playlist-modify",
    "playlist-modify-private",
    "playlist-modify-public",
    "playlist-read",
    "playlist-read-collaborative",
    "playlist-read-private",
    "streaming",
    "ugc-image-upload",
    "user-follow-modify",
    "user-follow-read",
    "user-library-modify",
    "user-library-read",
    "user-modify",
    "user-modify-playback-state",
    "user-modify-private",
    "user-personalized",
    "user-read-birthdate",
    "user-read-currently-playing",
    "user-read-email",
    "user-read-play-history",
    "user-read-playback-position",
    "user-read-playback-state",
    "user-read-private",
    "user-read-recently-played",
    "user-top-read",
];

/// The client id the official desktop client (and librespot) uses. Taken
/// from librespot's own `SessionConfig` so we never hard-code it.
pub fn desktop_client_id() -> String {
    librespot_core::SessionConfig::default().client_id
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub enum Bitrate {
    #[serde(rename = "96")]
    Low,
    #[serde(rename = "160")]
    #[default]
    Normal,
    #[serde(rename = "320")]
    High,
}

/// Values that may be set in `config.json`. All optional.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct FileConfig {
    pub redirect_port: Option<u16>,
    pub theme: Option<ThemeChoice>,
    pub bitrate: Option<Bitrate>,
    /// Expose Ferrotune as a Spotify Connect device (default true).
    pub connect: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Client id for login and the librespot session: the desktop client id
    /// unless the hidden `FERROTUNE_CLIENT_ID` override is set.
    pub client_id: String,
    pub redirect_port: u16,
    /// Announce a Spotify Connect device ("Ferrotune on <host>").
    pub connect: bool,
    pub device_name: String,
    pub theme: ThemeChoice,
    pub bitrate: Bitrate,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub state_dir: PathBuf,
    pub config_dir: PathBuf,
    pub accounts_base: String,
    /// spclient base override (tests / advanced). `None` = resolved by the
    /// librespot session's access-point resolver.
    pub spclient_base: Option<String>,
    pub pathfinder_base: String,
    /// `--mock`: no network, mock data, in-memory secrets.
    pub mock: bool,
    /// Log `first_frame_ms` (FERRO_TRACE_STARTUP=1).
    pub trace_startup: bool,
}

impl Config {
    /// Load configuration from the environment and the config file.
    pub fn load(mock: bool) -> Self {
        let (data_dir, cache_dir, state_dir, config_dir) = dirs();
        let file = read_file_config(&config_dir.join("config.json"));
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        Self::resolve(
            file,
            Env {
                client_id: env("FERROTUNE_CLIENT_ID"),
                port: env("FERROTUNE_REDIRECT_PORT").and_then(|p| p.parse().ok()),
                theme: env("FERRO_THEME"),
                connect: env("FERROTUNE_CONNECT").map(|v| v != "0"),
                spclient: env("FERROTUNE_SPCLIENT_URL"),
                pathfinder: env("FERROTUNE_PATHFINDER_URL"),
            },
            Paths {
                data_dir,
                cache_dir,
                state_dir,
                config_dir,
            },
            mock,
            env("FERRO_TRACE_STARTUP").is_some_and(|v| v == "1"),
        )
    }

    fn resolve(file: FileConfig, env: Env, paths: Paths, mock: bool, trace_startup: bool) -> Self {
        let theme = match env.theme.as_deref() {
            Some("light") => ThemeChoice::Light,
            Some("dark") => ThemeChoice::Dark,
            _ => file.theme.unwrap_or_default(),
        };
        Self {
            client_id: env
                .client_id
                .map(|c| c.trim().to_owned())
                .unwrap_or_else(desktop_client_id),
            redirect_port: env
                .port
                .or(file.redirect_port)
                .unwrap_or(DEFAULT_REDIRECT_PORT),
            connect: env.connect.or(file.connect).unwrap_or(true),
            device_name: device_name(hostname().as_deref()),
            theme,
            bitrate: file.bitrate.unwrap_or_default(),
            data_dir: paths.data_dir,
            cache_dir: paths.cache_dir,
            state_dir: paths.state_dir,
            config_dir: paths.config_dir,
            accounts_base: SPOTIFY_ACCOUNTS.into(),
            spclient_base: env.spclient.map(|u| u.trim_end_matches('/').to_owned()),
            pathfinder_base: env
                .pathfinder
                .map(|u| u.trim_end_matches('/').to_owned())
                .unwrap_or_else(|| SPOTIFY_PATHFINDER.into()),
            mock,
            trace_startup,
        }
    }

    /// The loopback redirect URI. The desktop client id accepts
    /// `http://127.0.0.1:<port>/login` (librespot uses the same form);
    /// Spotify requires the literal `127.0.0.1`, not `localhost`.
    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/login", self.redirect_port)
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("cache.sqlite3")
    }

    pub fn log_path(&self) -> PathBuf {
        self.state_dir.join("log")
    }

    /// True when the hidden `FERROTUNE_CLIENT_ID` override is in use.
    pub fn custom_client_id(&self) -> bool {
        self.client_id != desktop_client_id()
    }

    /// Optional overrides for GraphQL persisted-query hashes.
    pub fn pathfinder_overrides_path(&self) -> PathBuf {
        self.config_dir.join("pathfinder.json")
    }

    /// Configuration for tests: temp-free, mock, never touches real dirs.
    pub fn for_tests() -> Self {
        let tmp = std::env::temp_dir().join("ferrotune-tests");
        Self::resolve(
            FileConfig::default(),
            Env::default(),
            Paths {
                data_dir: tmp.clone(),
                cache_dir: tmp.clone(),
                state_dir: tmp.clone(),
                config_dir: tmp,
            },
            true,
            false,
        )
    }
}

#[derive(Default)]
struct Env {
    client_id: Option<String>,
    port: Option<u16>,
    theme: Option<String>,
    connect: Option<bool>,
    spclient: Option<String>,
    pathfinder: Option<String>,
}

/// Name shown in the Spotify Connect device picker.
pub fn device_name(host: Option<&str>) -> String {
    match host.map(str::trim).filter(|h| !h.is_empty()) {
        Some(h) => format!("{APP_NAME} on {h}"),
        None => APP_NAME.to_owned(),
    }
}

/// The machine's host name, without pulling in a crate for it.
fn hostname() -> Option<String> {
    if let Ok(h) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        return Some(h.trim().to_owned());
    }
    if let Ok(h) = std::fs::read_to_string("/etc/hostname") {
        return Some(h.trim().to_owned());
    }
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
}

struct Paths {
    data_dir: PathBuf,
    cache_dir: PathBuf,
    state_dir: PathBuf,
    config_dir: PathBuf,
}

fn dirs() -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    match directories::ProjectDirs::from("dev", "ferro", "ferrotune") {
        Some(p) => {
            let state = p
                .state_dir()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| p.data_local_dir().to_path_buf());
            (
                p.data_dir().to_path_buf(),
                p.cache_dir().to_path_buf(),
                state,
                p.config_dir().to_path_buf(),
            )
        }
        None => {
            let base = std::env::temp_dir().join("ferrotune");
            (base.clone(), base.clone(), base.clone(), base)
        }
    }
}

fn read_file_config(path: &Path) -> FileConfig {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "ignoring invalid config.json");
            FileConfig::default()
        }),
        Err(_) => FileConfig::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> Paths {
        Paths {
            data_dir: "/d".into(),
            cache_dir: "/c".into(),
            state_dir: "/s".into(),
            config_dir: "/cfg".into(),
        }
    }

    #[test]
    fn env_overrides_file() {
        let file: FileConfig = serde_json::from_str(
            r#"{"client_id":"ignored","redirect_port":9000,"theme":"light","connect":false}"#,
        )
        .unwrap();
        let c = Config::resolve(
            file,
            Env {
                client_id: Some(" env-id ".into()),
                ..Env::default()
            },
            paths(),
            false,
            false,
        );
        assert_eq!(c.client_id, "env-id");
        assert!(c.custom_client_id());
        assert_eq!(c.redirect_port, 9000);
        assert_eq!(c.theme, ThemeChoice::Light);
        assert!(!c.connect);
        assert_eq!(c.redirect_uri(), "http://127.0.0.1:9000/login");
    }

    #[test]
    fn defaults_need_no_configuration() {
        let c = Config::resolve(
            FileConfig::default(),
            Env {
                theme: Some("dark".into()),
                ..Env::default()
            },
            paths(),
            true,
            false,
        );
        // Zero configuration: the desktop client id from librespot.
        assert_eq!(c.client_id, "65b708073fc0480ea92a077233ca87bd");
        assert!(!c.custom_client_id());
        assert_eq!(c.redirect_uri(), "http://127.0.0.1:8898/login");
        assert!(c.connect);
        assert_eq!(c.spclient_base, None);
        assert_eq!(c.pathfinder_base, SPOTIFY_PATHFINDER);
        assert_eq!(c.redirect_port, DEFAULT_REDIRECT_PORT);
        assert_eq!(c.theme, ThemeChoice::Dark);
        assert_eq!(c.bitrate, Bitrate::Normal);
        assert_eq!(c.db_path(), PathBuf::from("/d/cache.sqlite3"));
    }

    #[test]
    fn device_names() {
        assert_eq!(device_name(Some("desk\n")), "Ferrotune on desk");
        assert_eq!(device_name(Some("desk")), "Ferrotune on desk");
        assert_eq!(device_name(Some("  ")), "Ferrotune");
        assert_eq!(device_name(None), "Ferrotune");
    }
}
