//! Runtime configuration: XDG paths, the Spotify client id and env overrides.
//!
//! Precedence: environment variables > `config.json` > defaults.
//! `config.json` lives at `$XDG_CONFIG_HOME/ferrotune/config.json`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const APP_ID: &str = "dev.ferro.ferrotune";
pub const APP_NAME: &str = "Ferrotune";
pub const DEFAULT_REDIRECT_PORT: u16 = 8898;
pub const SPOTIFY_ACCOUNTS: &str = "https://accounts.spotify.com";
pub const SPOTIFY_API: &str = "https://api.spotify.com/v1";

/// OAuth scopes requested at login. `streaming` is what librespot needs to
/// open an audio session with the same access token.
pub const SCOPES: &[&str] = &[
    "streaming",
    "user-read-private",
    "user-library-read",
    "user-library-modify",
    "playlist-read-private",
    "playlist-read-collaborative",
    "user-follow-read",
    "user-top-read",
    "user-read-recently-played",
];

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
    pub client_id: Option<String>,
    pub redirect_port: Option<u16>,
    pub theme: Option<ThemeChoice>,
    pub bitrate: Option<Bitrate>,
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Spotify application client id (public; PKCE needs no secret).
    pub client_id: Option<String>,
    pub redirect_port: u16,
    pub theme: ThemeChoice,
    pub bitrate: Bitrate,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub state_dir: PathBuf,
    pub config_dir: PathBuf,
    pub api_base: String,
    pub accounts_base: String,
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
            env("FERROTUNE_CLIENT_ID"),
            env("FERROTUNE_REDIRECT_PORT").and_then(|p| p.parse().ok()),
            env("FERRO_THEME"),
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

    fn resolve(
        file: FileConfig,
        env_client_id: Option<String>,
        env_port: Option<u16>,
        env_theme: Option<String>,
        paths: Paths,
        mock: bool,
        trace_startup: bool,
    ) -> Self {
        let theme = match env_theme.as_deref() {
            Some("light") => ThemeChoice::Light,
            Some("dark") => ThemeChoice::Dark,
            _ => file.theme.unwrap_or_default(),
        };
        Self {
            client_id: env_client_id
                .or(file.client_id)
                .map(|c| c.trim().to_owned()),
            redirect_port: env_port
                .or(file.redirect_port)
                .unwrap_or(DEFAULT_REDIRECT_PORT),
            theme,
            bitrate: file.bitrate.unwrap_or_default(),
            data_dir: paths.data_dir,
            cache_dir: paths.cache_dir,
            state_dir: paths.state_dir,
            config_dir: paths.config_dir,
            api_base: SPOTIFY_API.into(),
            accounts_base: SPOTIFY_ACCOUNTS.into(),
            mock,
            trace_startup,
        }
    }

    /// The loopback redirect URI that must be registered in the Spotify
    /// developer dashboard. Spotify requires `127.0.0.1` (not `localhost`).
    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/login", self.redirect_port)
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("cache.sqlite3")
    }

    pub fn log_path(&self) -> PathBuf {
        self.state_dir.join("log")
    }

    /// Configuration for tests: temp-free, mock, never touches real dirs.
    pub fn for_tests() -> Self {
        let tmp = std::env::temp_dir().join("ferrotune-tests");
        Self::resolve(
            FileConfig::default(),
            Some("test-client-id".into()),
            None,
            None,
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
        let file: FileConfig =
            serde_json::from_str(r#"{"client_id":"file-id","redirect_port":9000,"theme":"light"}"#)
                .unwrap();
        let c = Config::resolve(
            file,
            Some(" env-id ".into()),
            None,
            None,
            paths(),
            false,
            false,
        );
        assert_eq!(c.client_id.as_deref(), Some("env-id"));
        assert_eq!(c.redirect_port, 9000);
        assert_eq!(c.theme, ThemeChoice::Light);
        assert_eq!(c.redirect_uri(), "http://127.0.0.1:9000/login");
    }

    #[test]
    fn defaults() {
        let c = Config::resolve(
            FileConfig::default(),
            None,
            None,
            Some("dark".into()),
            paths(),
            true,
            false,
        );
        assert_eq!(c.client_id, None);
        assert_eq!(c.redirect_port, DEFAULT_REDIRECT_PORT);
        assert_eq!(c.theme, ThemeChoice::Dark);
        assert_eq!(c.bitrate, Bitrate::Normal);
        assert_eq!(c.db_path(), PathBuf::from("/d/cache.sqlite3"));
    }
}
