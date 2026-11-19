//! Download, install and update Spotify Soloist from Spotify's CDN.
//!
//! Spotify's terms say "Do not redistribute Spotify Soloist archives or
//! binaries directly", so Ferrotune never bundles it: the archive is
//! downloaded on the user's machine, from the official URL, the first time
//! it is needed (the same as the user following the getting-started guide).
//!
//! * The archive streams to `<root>/download.part` (size-capped), the
//!   binary is extracted next to the installed one, made `0755`, and must
//!   answer `--version` before it atomically replaces the old one.
//! * Updates: the docs only say "download the build for the same OS and
//!   architecture and replace the executable". There is no version
//!   endpoint, so Ferrotune asks the CDN with a conditional GET
//!   (`If-None-Match` / `If-Modified-Since` from the last download): `304`
//!   means nothing new, `200` is a new archive. A new build is only
//!   installed when it is not older than the one in use.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use super::archive::{self, BuildInfo, Caps};
use crate::error::{Error, Result};

/// Spotify's public download host (docs: "Downloads and updates").
pub const CDN_BASE: &str = "https://soloist-builds.spotifycdn.com";
/// Largest archive we download (the real one is far smaller).
pub const MAX_ARCHIVE: u64 = 200 * 1024 * 1024;
/// `--version` must answer within this time.
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

/// Archive name for this OS/architecture, or `None` when Spotify publishes
/// no build Ferrotune can install (Windows ships a .zip: not handled yet).
pub fn archive_name(os: &str, arch: &str) -> Option<&'static str> {
    Some(match (os, arch) {
        ("linux", "x86_64") => "soloist_release_x86_64.tar.gz",
        ("linux", "aarch64") => "soloist_release_arm64.tar.gz",
        ("linux", "arm") => "soloist_release_arm32.tar.gz",
        ("macos", "aarch64") => "soloist_release_osx_arm64.tar.gz",
        _ => return None,
    })
}

pub fn archive_url(base: &str, name: &str) -> String {
    format!("{}/{name}", base.trim_end_matches('/'))
}

/// Where everything lives. `root` is `$XDG_DATA_HOME/ferrotune/soloist`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Paths {
    pub root: PathBuf,
    /// Soloist's `--cache-dir` (under Ferrotune's cache dir).
    pub cache: PathBuf,
}

impl Paths {
    pub fn new(data_dir: &Path, cache_dir: &Path) -> Self {
        Self {
            root: data_dir.join("soloist"),
            cache: cache_dir.join("soloist"),
        }
    }

    pub fn binary(&self) -> PathBuf {
        self.root.join("bin").join("soloist")
    }

    /// Soloist's `--data-dir`: device identity and the paired session.
    pub fn data(&self) -> PathBuf {
        self.root.join("data")
    }

    fn meta(&self) -> PathBuf {
        self.root.join("build.json")
    }

    fn staging(&self) -> PathBuf {
        self.root.join("bin").join("soloist.new")
    }

    fn download(&self) -> PathBuf {
        self.root.join("download.part")
    }
}

/// What we know about the installed build (`build.json`, no secrets).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InstallMeta {
    pub version: Option<String>,
    pub built_unix: Option<u64>,
    /// Fallback "build date" when `--version` has none: the archive's
    /// `Last-Modified`, else the install time.
    pub fetched_unix: u64,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// Last time the CDN was asked for a newer build.
    pub checked_unix: u64,
    /// `--help` documents reading the key from `SOLOIST_API_KEY`.
    pub key_from_env: bool,
}

impl InstallMeta {
    pub fn build_unix(&self) -> u64 {
        self.built_unix.unwrap_or(self.fetched_unix)
    }

    pub fn expires_unix(&self) -> u64 {
        self.build_unix()
            .saturating_add(archive::BUILD_LIFETIME_DAYS * archive::DAY_SECS)
    }

    pub fn load(paths: &Paths) -> Option<Self> {
        let s = std::fs::read_to_string(paths.meta()).ok()?;
        serde_json::from_str(&s).ok()
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        let tmp = paths.meta().with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, paths.meta())?;
        Ok(())
    }
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// When to look for a newer build: daily, every 6 h in the last two weeks
/// before expiry, and at once when expired.
pub fn update_due(meta: &InstallMeta, now: u64) -> bool {
    let left = meta.expires_unix().saturating_sub(now);
    let every = if left == 0 {
        0
    } else if left < 14 * archive::DAY_SECS {
        6 * 3600
    } else {
        archive::DAY_SECS
    };
    now.saturating_sub(meta.checked_unix) >= every
}

/// Result of [`fetch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    /// `304 Not Modified`: the installed build is the newest.
    NotModified,
    /// A new binary was installed (or the first one).
    Installed(InstallMeta),
    /// The CDN has a build, but it is older than the installed one.
    Older,
}

/// Create the directories (private: the data dir holds the paired session).
pub fn prepare_dirs(paths: &Paths) -> Result<()> {
    for d in [paths.root.join("bin"), paths.data(), paths.cache.clone()] {
        std::fs::create_dir_all(&d)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for d in [&paths.root, &paths.data()] {
            std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

/// Download the archive (conditionally when `current` is installed) and
/// install the binary it contains. `binary_running` is told to stop before
/// the file is swapped by the caller (the swap itself is an atomic rename,
/// so a running process keeps its old inode).
pub async fn fetch(
    http: &reqwest::Client,
    url: &str,
    paths: &Paths,
    current: Option<&InstallMeta>,
) -> Result<Fetched> {
    prepare_dirs(paths)?;
    let mut req = http.get(url);
    if let Some(m) = current.filter(|_| paths.binary().exists()) {
        if let Some(e) = &m.etag {
            req = req.header(reqwest::header::IF_NONE_MATCH, e);
        }
        if let Some(l) = &m.last_modified {
            req = req.header(reqwest::header::IF_MODIFIED_SINCE, l);
        }
    }
    let resp = req.send().await?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Fetched::NotModified);
    }
    if !status.is_success() {
        return Err(Error::Http {
            status: status.as_u16(),
            message: "Spotify's download server refused the request".into(),
        });
    }
    if resp.content_length().is_some_and(|n| n > MAX_ARCHIVE) {
        return Err(Error::Protocol("the Soloist archive is too large".into()));
    }
    let header = |h: reqwest::header::HeaderName| {
        resp.headers()
            .get(h)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.chars().take(256).collect::<String>())
    };
    let etag = header(reqwest::header::ETAG);
    let last_modified = header(reqwest::header::LAST_MODIFIED);
    let fetched_unix = last_modified
        .as_deref()
        .and_then(|l| httpdate::parse_http_date(l).ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or_else(now_unix, |d| d.as_secs());

    // Stream to disk with a cap.
    let part = paths.download();
    let mut file = tokio::fs::File::create(&part).await?;
    let mut resp = resp;
    let mut total: u64 = 0;
    while let Some(chunk) = resp.chunk().await? {
        total = total.saturating_add(chunk.len() as u64);
        if total > MAX_ARCHIVE {
            drop(file);
            let _ = tokio::fs::remove_file(&part).await;
            return Err(Error::Protocol("the Soloist archive is too large".into()));
        }
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file);

    let staging = paths.staging();
    let extracted = {
        let (part, staging) = (part.clone(), staging.clone());
        tokio::task::spawn_blocking(move || extract_to(&part, &staging))
            .await
            .map_err(|_| Error::Cancelled)?
    };
    let _ = tokio::fs::remove_file(&part).await;
    extracted?;

    let info = match probe(&staging).await {
        Ok(i) => i,
        Err(e) => {
            let _ = tokio::fs::remove_file(&staging).await;
            return Err(e);
        }
    };
    let new_build = info.version.built_unix.unwrap_or(fetched_unix);
    if let Some(cur) = current
        && paths.binary().exists()
        && new_build < cur.build_unix()
    {
        let _ = tokio::fs::remove_file(&staging).await;
        return Ok(Fetched::Older);
    }
    tokio::fs::rename(&staging, paths.binary()).await?;
    let meta = InstallMeta {
        version: info.version.version,
        built_unix: info.version.built_unix,
        fetched_unix,
        etag,
        last_modified,
        checked_unix: now_unix(),
        key_from_env: info.key_from_env,
    };
    meta.save(paths)?;
    Ok(Fetched::Installed(meta))
}

fn extract_to(archive_path: &Path, out_path: &Path) -> Result<()> {
    let input = std::io::BufReader::new(std::fs::File::open(archive_path)?);
    let mut out = std::fs::File::create(out_path)?;
    let r = archive::extract_binary(input, &mut out, Caps::default());
    drop(out);
    match r {
        Ok(_) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(out_path, std::fs::Permissions::from_mode(0o755))?;
            }
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(out_path);
            Err(Error::Protocol(format!("Soloist download: {e}")))
        }
    }
}

/// What a freshly extracted binary says about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probe {
    pub version: BuildInfo,
    pub key_from_env: bool,
}

/// Run `--version` (must succeed) and `--help` (best effort) with a
/// timeout. The help text decides whether the API key can be passed in the
/// environment instead of argv.
pub async fn probe(bin: &Path) -> Result<Probe> {
    let out = run_capture(bin, "--version").await?;
    if !out.0 {
        return Err(Error::Protocol(
            "the downloaded Soloist does not run on this computer".into(),
        ));
    }
    let help = run_capture(bin, "--help")
        .await
        .map(|h| h.1)
        .unwrap_or_default();
    Ok(Probe {
        version: archive::parse_version(&out.1),
        key_from_env: help_reads_env_key(&help),
    })
}

/// True when `--help` says the key can come from `SOLOIST_API_KEY` (e.g.
/// clap's `[env: SOLOIST_API_KEY=]`). The docs only show `--api-key`.
pub fn help_reads_env_key(help: &str) -> bool {
    help.lines().any(|l| {
        let l = l.to_ascii_lowercase();
        l.contains("soloist_api_key") && (l.contains("env") || l.contains("environment"))
    })
}

async fn run_capture(bin: &Path, arg: &str) -> Result<(bool, String)> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg(arg)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env_remove("SOLOIST_API_KEY")
        .kill_on_drop(true);
    let out = tokio::time::timeout(VERSION_TIMEOUT, cmd.output())
        .await
        .map_err(|_| Error::Protocol("Soloist did not answer --version".into()))?
        .map_err(|e| Error::Io(format!("could not run Soloist: {e}")))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    text.truncate(64 * 1024);
    Ok((out.status.success(), text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_names_follow_the_download_table() {
        assert_eq!(
            archive_name("linux", "x86_64"),
            Some("soloist_release_x86_64.tar.gz")
        );
        assert_eq!(
            archive_name("linux", "aarch64"),
            Some("soloist_release_arm64.tar.gz")
        );
        assert_eq!(
            archive_name("linux", "arm"),
            Some("soloist_release_arm32.tar.gz")
        );
        assert_eq!(
            archive_name("macos", "aarch64"),
            Some("soloist_release_osx_arm64.tar.gz")
        );
        assert_eq!(archive_name("windows", "x86_64"), None);
        assert_eq!(
            archive_url("https://soloist-builds.spotifycdn.com/", "a.tar.gz"),
            "https://soloist-builds.spotifycdn.com/a.tar.gz"
        );
    }

    #[test]
    fn update_schedule() {
        let day = archive::DAY_SECS;
        let meta = InstallMeta {
            built_unix: Some(0),
            checked_unix: 10 * day,
            ..InstallMeta::default()
        };
        assert_eq!(meta.expires_unix(), 90 * day);
        assert!(!update_due(&meta, 10 * day + 3600));
        assert!(update_due(&meta, 11 * day));
        // Last two weeks: every 6 hours.
        let meta = InstallMeta {
            checked_unix: 80 * day,
            ..meta
        };
        assert!(!update_due(&meta, 80 * day + 3600));
        assert!(update_due(&meta, 80 * day + 6 * 3600));
        // Expired: always.
        let meta = InstallMeta {
            checked_unix: 91 * day,
            ..meta
        };
        assert!(update_due(&meta, 91 * day));
        // No build date: the fetch date stands in.
        let meta = InstallMeta {
            fetched_unix: 5 * day,
            ..InstallMeta::default()
        };
        assert_eq!(meta.expires_unix(), 95 * day);
    }

    #[test]
    fn env_key_detection() {
        assert!(help_reads_env_key(
            "  -k, --api-key <KEY>  API key [env: SOLOIST_API_KEY=]"
        ));
        assert!(!help_reads_env_key(
            "  -k, --api-key KEY  Spotify Soloist API key"
        ));
        assert!(!help_reads_env_key(""));
    }
}
