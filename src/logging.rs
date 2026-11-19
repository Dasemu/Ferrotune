//! `tracing` setup with a redaction layer (docs/architecture.md §6).
//!
//! Every formatted log line goes through [`redact_line`] before it reaches
//! stderr or the rotated log file, so a token that slips into a `Debug`
//! impl of a third-party type (e.g. an HTTP error with a URL) is masked.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;

/// Max size of the log file before it is rotated to `log.1`.
pub const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

const MASK: &str = "[REDACTED]";

/// Keys whose values are masked in `key=value`, `key: value`,
/// `"key":"value"` and `"key": "value"` forms. Matched on a word boundary,
/// so `barcode=` or `zip_code=` are left alone.
const KEYS: &[&str] = &[
    "access_token",
    "accessToken",
    "refresh_token",
    "client-token",
    "client_token",
    "code_verifier",
    "code",
    "auth_data",
    "client_secret",
    "password",
];

/// Prefixes after which the following token is masked.
const PREFIXES: &[&str] = &["Bearer ", "bearer "];

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~' | '+' | '/' | '=' | '%')
}

fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn token_len(s: &str) -> usize {
    s.char_indices()
        .find(|(_, c)| !is_token_char(*c))
        .map_or(s.len(), |(i, _)| i)
}

/// If `key` occurs at `i` as a whole word followed by a separator, return
/// the byte offset where its value starts.
fn value_start(line: &str, i: usize, key: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    if i > 0 && is_word_char(bytes[i - 1]) {
        return None;
    }
    let mut j = i + key.len();
    if j < bytes.len() && is_word_char(bytes[j]) {
        return None;
    }
    if bytes.get(j) == Some(&b'"') {
        j += 1;
    }
    while bytes.get(j) == Some(&b' ') {
        j += 1;
    }
    if !matches!(bytes.get(j), Some(b'=' | b':')) {
        return None;
    }
    j += 1;
    while bytes.get(j) == Some(&b' ') {
        j += 1;
    }
    if bytes.get(j) == Some(&b'"') {
        j += 1;
    }
    Some(j)
}

/// Mask secrets in a single formatted log line.
pub fn redact_line(line: &str) -> String {
    // Collect (start, end) ranges of secret values, then rebuild the line.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for key in KEYS {
        let mut from = 0;
        while let Some(off) = line[from..].find(key) {
            let i = from + off;
            from = i + key.len();
            if let Some(v) = value_start(line, i, key) {
                let len = token_len(&line[v..]);
                if len > 0 {
                    ranges.push((v, v + len));
                }
            }
        }
    }
    for p in PREFIXES {
        let mut from = 0;
        while let Some(off) = line[from..].find(p) {
            let v = from + off + p.len();
            from = v;
            let len = token_len(&line[v..]);
            if len > 0 {
                ranges.push((v, v + len));
            }
        }
    }
    if ranges.is_empty() {
        return line.to_owned();
    }
    ranges.sort_unstable();
    let mut out = String::with_capacity(line.len());
    let mut pos = 0;
    for (start, end) in ranges {
        if end <= pos {
            continue;
        }
        let start = start.max(pos);
        out.push_str(&line[pos..start]);
        out.push_str(MASK);
        pos = end;
    }
    out.push_str(&line[pos..]);
    out
}

/// A size-capped log file that rotates to `<name>.1`.
struct RotatingFile {
    path: PathBuf,
    file: Option<File>,
    written: u64,
}

impl RotatingFile {
    fn open(path: &Path) -> Self {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let file = OpenOptions::new().create(true).append(true).open(path).ok();
        let written = file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .map_or(0, |m| m.len());
        Self {
            path: path.to_path_buf(),
            file,
            written,
        }
    }

    fn write_all(&mut self, buf: &[u8]) {
        if self.written + buf.len() as u64 > MAX_LOG_BYTES {
            let rotated = self.path.with_extension("1");
            self.file = None;
            let _ = std::fs::rename(&self.path, rotated);
            self.file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .ok();
            self.written = 0;
        }
        if let Some(f) = self.file.as_mut()
            && f.write_all(buf).is_ok()
        {
            self.written += buf.len() as u64;
        }
    }
}

/// Writer handed to `tracing_subscriber`: buffers one event, redacts it and
/// fans it out to stderr and the log file.
#[derive(Clone)]
pub struct RedactingMakeWriter {
    file: Option<Arc<Mutex<RotatingFile>>>,
    sink: Option<Arc<Mutex<Vec<u8>>>>,
    stderr: bool,
}

impl RedactingMakeWriter {
    /// A writer that captures into memory; used by the redaction test.
    pub fn capture() -> (Self, Arc<Mutex<Vec<u8>>>) {
        let buf = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                file: None,
                sink: Some(buf.clone()),
                stderr: false,
            },
            buf,
        )
    }
}

pub struct EventWriter {
    buf: Vec<u8>,
    target: RedactingMakeWriter,
}

impl Write for EventWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for EventWriter {
    fn drop(&mut self) {
        let text = String::from_utf8_lossy(&self.buf);
        let redacted = redact_line(&text);
        if self.target.stderr {
            let _ = io::stderr().write_all(redacted.as_bytes());
        }
        if let Some(file) = &self.target.file
            && let Ok(mut f) = file.lock()
        {
            f.write_all(redacted.as_bytes());
        }
        if let Some(sink) = &self.target.sink
            && let Ok(mut s) = sink.lock()
        {
            s.extend_from_slice(redacted.as_bytes());
        }
    }
}

impl<'a> MakeWriter<'a> for RedactingMakeWriter {
    type Writer = EventWriter;

    fn make_writer(&'a self) -> Self::Writer {
        EventWriter {
            buf: Vec::with_capacity(256),
            target: self.clone(),
        }
    }
}

/// Initialise global logging. Safe to call once; later calls are ignored.
pub fn init(log_path: Option<&Path>) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        // librespot is chatty at info level and logs usernames.
        EnvFilter::new("info,librespot=warn,librespot_core=warn,librespot_playback=warn")
    });
    let writer = RedactingMakeWriter {
        file: log_path.map(|p| Arc::new(Mutex::new(RotatingFile::open(p)))),
        sink: None,
        stderr: true,
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_known_patterns() {
        let l = redact_line("GET /v1/me Authorization: Bearer BQDabc-123_x.y end");
        assert_eq!(l, "GET /v1/me Authorization: Bearer [REDACTED] end");
        let l = redact_line("redirect /login?code=AQBxyz&state=s1 refresh_token=AQ12");
        assert!(!l.contains("AQBxyz"));
        assert!(!l.contains("AQ12"));
        assert!(l.contains("state=s1"));
        let l = redact_line(r#"{"access_token":"tok1","token_type":"Bearer"}"#);
        assert!(!l.contains("tok1"));
        let l = redact_line(r#"{"access_token": "tok2", "refresh_token" : "tok3"}"#);
        assert!(!l.contains("tok2") && !l.contains("tok3"), "{l}");
        let l = redact_line("TokenSet { access_token: \"tok4\", code: abc }");
        assert!(!l.contains("tok4") && !l.contains("abc"), "{l}");
        // Word boundary: other keys ending in "code" are not secrets.
        let l = redact_line("scanned barcode=12345 zip_code=10115 code=AQsecret");
        assert!(
            l.contains("barcode=12345") && l.contains("zip_code=10115"),
            "{l}"
        );
        assert!(!l.contains("AQsecret"));
    }

    #[test]
    fn formatted_log_output_never_contains_secrets() {
        use crate::auth::oauth::TokenSet;
        use crate::auth::secrets::Secret;
        let (writer, buf) = RedactingMakeWriter::capture();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer)
            .with_ansi(false)
            .finish();
        let tokens = TokenSet {
            access_token: Secret::new("BQDsecretAccess123"),
            expires_at: 0,
            scope: String::new(),
        };
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(?tokens, "token debug");
            // A third-party error that embeds a URL with a code and a header.
            tracing::warn!("callback GET /login?code=AQCsecretCode789&state=x");
            tracing::warn!("sent Authorization: Bearer BQDsecretAccess123");
            tracing::warn!("client-token: AQDsecretRefresh456");
            tracing::info!(body = %crate::error::Redacted("my private search"), "search");
        });
        let out = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        for secret in [
            "BQDsecretAccess123",
            "AQDsecretRefresh456",
            "AQCsecretCode789",
            "my private search",
        ] {
            assert!(!out.contains(secret), "leaked {secret} in:\n{out}");
        }
        assert!(out.contains("token debug"));
    }

    #[test]
    fn leaves_plain_lines_alone() {
        let l = "loaded 42 playlists in 12ms";
        assert_eq!(redact_line(l), l);
    }

    #[test]
    fn rotation_caps_size() {
        let dir = std::env::temp_dir().join(format!("ferrotune-log-{}", std::process::id()));
        let path = dir.join("log");
        let mut f = RotatingFile::open(&path);
        let chunk = vec![b'x'; 1024 * 1024];
        for _ in 0..6 {
            f.write_all(&chunk);
        }
        assert!(f.written <= MAX_LOG_BYTES);
        assert!(path.with_extension("1").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
