//! Running the `soloist` daemon as a child process.
//!
//! * [`build_command`] is pure: argv for a [`LaunchSpec`]. The API key goes
//!   into `--api-key`: Soloist 1.3.10's `--help` lists no environment
//!   variable for it, so it is visible in `/proc/<pid>/cmdline` to
//!   processes of the same user (never logged: see [`printable_args`]).
//! * stdout/stderr are read line by line (capped), redacted (the key, plus
//!   the app-wide token scrubber) and logged at debug level.
//! * On Linux the child gets `PR_SET_PDEATHSIG`, so it dies with Ferrotune
//!   even after a crash; [`terminate`] sends SIGTERM and waits before
//!   SIGKILL, so Soloist can remove its runtime files.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};

use crate::auth::secrets::Secret;

/// Default `--cache-size` in MB (Soloist's own default is unlimited).
pub const DEFAULT_CACHE_MB: u32 = 500;
/// Soloist refuses a cache below 100 MB (`0` = unlimited, never used).
pub const MIN_CACHE_MB: u32 = 100;
/// Soloist's exit code for an expired build.
pub const EXIT_EXPIRED: i32 = 10;
/// Longest log line kept (Soloist output is untrusted).
const MAX_LINE: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LaunchSpec {
    pub binary: PathBuf,
    pub device_name: String,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    /// PipeWire node name or id; `None` = the system default output.
    pub pipewire_device: Option<String>,
    /// `--cache-size` in MB (clamped to at least [`MIN_CACHE_MB`]).
    pub cache_mb: u32,
    /// `--initial-volume` 0–100 (Soloist's default is 40).
    pub initial_volume: Option<u8>,
    /// `--pair`: store a new session and exit.
    pub pair: bool,
}

/// Argv (without the program).
#[derive(Debug, PartialEq, Eq)]
pub struct Invocation {
    pub args: Vec<OsString>,
}

pub fn build_command(spec: &LaunchSpec, key: &Secret) -> Invocation {
    let mut args: Vec<OsString> = vec![
        "--device-name".into(),
        spec.device_name.clone().into(),
        "--data-dir".into(),
        spec.data_dir.clone().into_os_string(),
        "--cache-dir".into(),
        spec.cache_dir.clone().into_os_string(),
        "--cache-size".into(),
        spec.cache_mb.max(MIN_CACHE_MB).to_string().into(),
    ];
    if !spec.pair {
        // Local control only, on a port the OS picks (written back to
        // `<data-dir>/ws.port`).
        args.push("--ws".into());
        args.push("127.0.0.1:0".into());
        if let Some(v) = spec.initial_volume {
            args.push("--initial-volume".into());
            args.push(v.min(100).to_string().into());
        }
    }
    if let Some(d) = spec
        .pipewire_device
        .as_deref()
        .filter(|d| !d.trim().is_empty())
    {
        args.push("--pipewire-device".into());
        args.push(d.trim().into());
    }
    if spec.pair {
        args.push("--pair".into());
    }
    args.push("--api-key".into());
    args.push(key.expose().into());
    Invocation { args }
}

/// The argv as it may be logged.
pub fn printable_args(inv: &Invocation, key: &Secret) -> String {
    inv.args
        .iter()
        .map(|a| redact(&a.to_string_lossy(), key))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Remove the key (and anything token-shaped) from a line of output.
pub fn redact(line: &str, key: &Secret) -> String {
    let k = key.expose();
    let line = if k.len() >= 4 {
        line.replace(k, "[REDACTED]")
    } else {
        line.to_owned()
    };
    crate::logging::redact_line(&line)
}

/// Spawn the daemon. stdin is closed; stdout and stderr are piped.
pub fn spawn(spec: &LaunchSpec, key: &Secret) -> std::io::Result<Child> {
    let inv = build_command(spec, key);
    let mut cmd = Command::new(&spec.binary);
    cmd.args(&inv.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(target_os = "linux")]
    // SAFETY: only async-signal-safe libc calls between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    tracing::debug!(args = %printable_args(&inv, key), "starting soloist");
    cmd.spawn()
}

/// Log a child's output stream (capped, redacted). Returns when it closes.
pub async fn pipe_logs<R: AsyncRead + Unpin>(r: R, key: Secret, stream: &'static str) {
    let mut reader = BufReader::new(r);
    let mut buf = Vec::with_capacity(256);
    loop {
        buf.clear();
        // read_until without a cap could grow without bound: read in
        // bounded pieces and drop the rest of an over-long line.
        let mut truncated = false;
        loop {
            let chunk = match reader.fill_buf().await {
                Ok(c) => c,
                Err(_) => return,
            };
            if chunk.is_empty() {
                if buf.is_empty() {
                    return;
                }
                break;
            }
            let (line_end, used) = match chunk.iter().position(|b| *b == b'\n') {
                Some(i) => (true, i + 1),
                None => (false, chunk.len()),
            };
            let room = MAX_LINE.saturating_sub(buf.len());
            buf.extend_from_slice(&chunk[..used.min(room)]);
            truncated |= used > room;
            reader.consume(used);
            if line_end {
                break;
            }
        }
        let text = String::from_utf8_lossy(&buf);
        let text = text.trim_end();
        if !text.is_empty() {
            let line = redact(text, &key);
            tracing::debug!(target: "soloist", stream, truncated, "{line}");
        }
    }
}

/// Ask the child to stop (SIGTERM), then kill it after `grace`.
pub async fn terminate(child: &mut Child, grace: Duration) -> Option<i32> {
    #[cfg(unix)]
    if let Some(pid) = child.id().and_then(|p| i32::try_from(p).ok()) {
        // SAFETY: plain kill(2) on our own child's pid.
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
        if let Ok(Ok(st)) = tokio::time::timeout(grace, child.wait()).await {
            return st.code();
        }
    }
    let _ = child.kill().await;
    child.wait().await.ok().and_then(|s| s.code())
}

/// Resident memory of a process in bytes (`VmRSS` in /proc), Linux only.
pub fn rss_bytes(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    parse_vm_rss(&status)
}

pub fn parse_vm_rss(status: &str) -> Option<u64> {
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// A daemon left over from a previous Ferrotune run (crash, kill -9)
/// still holds the data directory: Soloist refuses a second instance on
/// it. Stop it, but only if `soloist.pid` points at *our* binary.
pub async fn stop_stale(data_dir: &Path, binary: &Path) {
    let Some(pid) = std::fs::read_to_string(data_dir.join("soloist.pid"))
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
        .filter(|p| *p > 1)
    else {
        return;
    };
    let ours = std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .zip(std::fs::canonicalize(binary).ok())
        .is_some_and(|(exe, bin)| exe == bin);
    if !ours {
        return;
    }
    tracing::info!("stopping a Soloist left over from a previous run");
    #[cfg(unix)]
    // SAFETY: kill(2) on a pid whose executable is our own Soloist binary.
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
    for _ in 0..30 {
        if !Path::new(&format!("/proc/{pid}")).exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    #[cfg(unix)]
    // SAFETY: as above; it ignored SIGTERM for 3 s.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
}

/// Restart delay after the n-th crash in a row: 1 s, 2 s, 4 s … 60 s.
pub fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(1u64 << attempt.min(6)).min(Duration::from_secs(60))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> LaunchSpec {
        LaunchSpec {
            binary: "/x/soloist".into(),
            device_name: "Ferrotune on desk".into(),
            data_dir: "/d".into(),
            cache_dir: "/c".into(),
            pipewire_device: None,
            cache_mb: DEFAULT_CACHE_MB,
            initial_volume: Some(70),
            pair: false,
        }
    }

    fn strs(inv: &Invocation) -> Vec<String> {
        inv.args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn argv_follows_the_command_line_reference() {
        let key = Secret::new("k3y-secret-value");
        let inv = build_command(&spec(), &key);
        assert_eq!(
            strs(&inv),
            [
                "--device-name",
                "Ferrotune on desk",
                "--data-dir",
                "/d",
                "--cache-dir",
                "/c",
                "--cache-size",
                "500",
                "--ws",
                "127.0.0.1:0",
                "--initial-volume",
                "70",
                "--api-key",
                "k3y-secret-value"
            ]
        );
        let printed = printable_args(&inv, &key);
        assert!(!printed.contains("k3y-secret-value"), "{printed}");
        assert!(printed.contains("--api-key [REDACTED]"));
    }

    #[test]
    fn pairing_and_options() {
        let key = Secret::new("k3y-secret-value");
        let inv = build_command(
            &LaunchSpec {
                pipewire_device: Some(" alsa_output.usb ".into()),
                cache_mb: 0,
                pair: true,
                ..spec()
            },
            &key,
        );
        let a = strs(&inv);
        // Pairing: no WebSocket server, no initial volume.
        assert!(!a.contains(&"--ws".to_owned()));
        assert!(!a.contains(&"--initial-volume".to_owned()));
        // Never an unlimited cache.
        let i = a.iter().position(|s| s == "--cache-size").unwrap();
        assert_eq!(a[i + 1], "100");
        assert!(a.ends_with(&[
            "--pipewire-device".into(),
            "alsa_output.usb".into(),
            "--pair".into(),
            "--api-key".into(),
            "k3y-secret-value".into()
        ]));
        // A blank device is the default output.
        let inv = build_command(
            &LaunchSpec {
                pipewire_device: Some("  ".into()),
                ..spec()
            },
            &key,
        );
        assert!(!strs(&inv).contains(&"--pipewire-device".to_owned()));
    }

    #[test]
    fn output_is_redacted() {
        let key = Secret::new("abcd1234");
        assert_eq!(
            redact("login with key abcd1234 failed", &key),
            "login with key [REDACTED] failed"
        );
        assert!(!redact("Bearer BQDxyz-123", &key).contains("BQDxyz"));
    }

    #[test]
    fn rss_parse_and_backoff() {
        let s = "Name:\tsoloist\nVmPeak:\t  9000 kB\nVmRSS:\t   41236 kB\n";
        assert_eq!(parse_vm_rss(s), Some(41_236 * 1024));
        assert_eq!(parse_vm_rss("Name: x"), None);
        assert_eq!(backoff(0), Duration::from_secs(1));
        assert_eq!(backoff(3), Duration::from_secs(8));
        assert_eq!(backoff(40), Duration::from_secs(60));
    }
}
