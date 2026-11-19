//! OAuth 2.0 Authorization Code flow with PKCE (RFC 7636) against the
//! Spotify accounts service, with a loopback redirect listener.
//!
//! No client secret is involved: the client id is public and the PKCE
//! verifier proves that the token request comes from the same process that
//! opened the browser.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::secrets::Secret;
use crate::error::{Error, Result};

const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";

/// Refresh the access token this many seconds before it expires.
pub const EXPIRY_MARGIN_SECS: u64 = 60;

/// PKCE verifier + S256 challenge.
#[derive(Debug, Clone)]
pub struct Pkce {
    pub verifier: Secret,
    pub challenge: String,
}

impl Pkce {
    /// Generate a fresh 64-character verifier (RFC 7636 allows 43..=128).
    pub fn generate() -> Self {
        let mut rng = rand::rng();
        let verifier: String = (0..64)
            .map(|_| UNRESERVED[rng.random_range(0..UNRESERVED.len())] as char)
            .collect();
        Self::from_verifier(verifier)
    }

    pub fn from_verifier(verifier: String) -> Self {
        let challenge = challenge_for(&verifier);
        Self {
            verifier: Secret::new(verifier),
            challenge,
        }
    }
}

/// `BASE64URL-ENCODE(SHA256(ASCII(code_verifier)))` without padding.
pub fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Random opaque `state` value for CSRF protection.
pub fn random_state() -> String {
    let mut rng = rand::rng();
    let bytes: [u8; 16] = rng.random();
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Build the `/authorize` URL the user opens in a browser.
pub fn authorize_url(
    accounts_base: &str,
    client_id: &str,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
    scopes: &[&str],
) -> Result<String> {
    let mut url = url::Url::parse(&format!("{accounts_base}/authorize"))
        .map_err(|e| Error::auth(format!("bad accounts url: {e}")))?;
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("code_challenge_method", "S256")
        .append_pair("code_challenge", challenge)
        .append_pair("state", state)
        .append_pair("scope", &scopes.join(" "));
    Ok(url.to_string())
}

/// What one request on the loopback listener means for the login.
#[derive(Debug)]
pub enum Callback {
    /// The authorization code: login can continue.
    Code(Secret),
    /// Not (validly) ours: answer with this status and keep listening.
    Ignore(&'static str),
    /// Spotify reported an error for *our* state (e.g. the user denied).
    Failed(Error),
}

/// Parse the HTTP request line the browser sends to the loopback listener
/// (`GET /login?code=..&state=.. HTTP/1.1`).
///
/// Anything that does not carry our `state` is ignored (400) so a stray or
/// hostile local request cannot abort the login.
pub fn parse_callback(request_line: &str, expected_state: &str) -> Callback {
    let mut parts = request_line.split_whitespace();
    let (Some("GET"), Some(target)) = (parts.next(), parts.next()) else {
        return Callback::Ignore("400 Bad Request");
    };
    let Ok(url) = url::Url::parse(&format!("http://127.0.0.1{target}")) else {
        return Callback::Ignore("400 Bad Request");
    };
    if url.path() != "/login" {
        // e.g. /favicon.ico
        return Callback::Ignore("404 Not Found");
    }
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            "error" => error = Some(v.into_owned()),
            _ => {}
        }
    }
    if state.as_deref() != Some(expected_state) {
        return Callback::Ignore("400 Bad Request");
    }
    if let Some(e) = error {
        return Callback::Failed(Error::auth(if e == "access_denied" {
            "Login was cancelled in the browser.".to_owned()
        } else {
            format!("Spotify rejected the login ({e}).")
        }));
    }
    match code {
        Some(c) if !c.is_empty() => Callback::Code(Secret::new(c)),
        _ => Callback::Ignore("400 Bad Request"),
    }
}

const CALLBACK_HTML: &str = "<!doctype html><html><head><meta charset=utf-8><title>Ferrotune</title>\
<style>body{background:#121212;color:#fff;font-family:sans-serif;display:flex;align-items:center;\
justify-content:center;height:100vh;margin:0}</style></head><body><div><h1>You're logged in</h1>\
<p>You can close this tab and return to Ferrotune.</p></div></body></html>";

/// Per-connection limit for the browser to send its request line.
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Bind the loopback listener. Call this BEFORE opening the browser so the
/// redirect can never race the listener (and a busy port fails early).
pub fn bind_callback(port: u16) -> Result<std::net::TcpListener> {
    let l = std::net::TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| Error::auth(format!("Can't listen on port {port}: {e}")))?;
    l.set_nonblocking(true)?;
    Ok(l)
}

/// Wait on an already bound listener until the browser delivers the code.
pub async fn wait_for_callback(
    listener: std::net::TcpListener,
    expected_state: String,
    timeout: Duration,
) -> Result<Secret> {
    let listener = TcpListener::from_std(listener)?;
    tokio::time::timeout(timeout, accept_loop(listener, &expected_state))
        .await
        .map_err(|_| Error::auth("Timed out waiting for the browser login."))?
}

async fn read_request_line(sock: &mut tokio::net::TcpStream) -> Option<String> {
    let mut buf = vec![0u8; 8192];
    let mut len = 0;
    while len < buf.len() {
        let n = sock.read(&mut buf[len..]).await.ok()?;
        if n == 0 {
            break;
        }
        len += n;
        if buf[..len].windows(2).any(|w| w == b"\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf[..len]);
    text.lines().next().map(str::to_owned)
}

async fn accept_loop(listener: TcpListener, expected_state: &str) -> Result<Secret> {
    loop {
        let Ok((mut sock, _)) = listener.accept().await else {
            continue;
        };
        // A client that connects and sends nothing must not block the login.
        let line = tokio::time::timeout(READ_TIMEOUT, read_request_line(&mut sock))
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        let result = parse_callback(&line, expected_state);
        let (status, body) = match &result {
            Callback::Code(_) => ("200 OK", CALLBACK_HTML.to_owned()),
            Callback::Ignore(status) => (*status, String::new()),
            Callback::Failed(e) => (
                "400 Bad Request",
                format!("Login failed: {}", e.user_message()),
            ),
        };
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = tokio::time::timeout(READ_TIMEOUT, sock.write_all(resp.as_bytes())).await;
        let _ = sock.shutdown().await;
        match result {
            Callback::Code(code) => return Ok(code),
            Callback::Ignore(_) => continue,
            Callback::Failed(e) => return Err(e),
        }
    }
}

/// Access + refresh token pair. Serialised only into the keyring.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenSet {
    pub access_token: Secret,
    pub refresh_token: Option<Secret>,
    /// Unix seconds.
    pub expires_at: u64,
    pub scope: String,
}

#[derive(Serialize, Deserialize)]
struct StoredTokens {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: u64,
    #[serde(default)]
    scope: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: u64,
    #[serde(default)]
    scope: String,
}

#[derive(Deserialize)]
struct TokenError {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl TokenSet {
    pub fn is_expired(&self, now: u64) -> bool {
        now.saturating_add(EXPIRY_MARGIN_SECS) >= self.expires_at
    }

    pub fn to_secret(&self) -> Result<Secret> {
        let s = StoredTokens {
            access_token: self.access_token.expose().to_owned(),
            refresh_token: self.refresh_token.as_ref().map(|r| r.expose().to_owned()),
            expires_at: self.expires_at,
            scope: self.scope.clone(),
        };
        Ok(Secret::new(serde_json::to_string(&s)?))
    }

    pub fn from_secret(secret: &Secret) -> Result<Self> {
        let s: StoredTokens = serde_json::from_str(secret.expose())
            .map_err(|_| Error::auth("stored token is unreadable"))?;
        Ok(Self {
            access_token: Secret::new(s.access_token),
            refresh_token: s.refresh_token.map(Secret::new),
            expires_at: s.expires_at,
            scope: s.scope,
        })
    }

    fn from_response(r: TokenResponse, previous_refresh: Option<&Secret>, now: u64) -> Self {
        Self {
            access_token: Secret::new(r.access_token),
            refresh_token: r
                .refresh_token
                .map(Secret::new)
                .or_else(|| previous_refresh.cloned()),
            expires_at: now.saturating_add(r.expires_in),
            scope: r.scope,
        }
    }
}

async fn token_request(
    http: &reqwest::Client,
    accounts_base: &str,
    form: &[(&str, &str)],
    previous_refresh: Option<&Secret>,
) -> Result<TokenSet> {
    let resp = http
        .post(format!("{accounts_base}/api/token"))
        .form(form)
        .send()
        .await?;
    let status = resp.status();
    let body = resp.bytes().await?;
    if status.is_success() {
        let r: TokenResponse = serde_json::from_slice(&body)
            .map_err(|_| Error::Protocol("token response was not valid JSON".into()))?;
        return Ok(TokenSet::from_response(r, previous_refresh, now_secs()));
    }
    let err: Option<TokenError> = serde_json::from_slice(&body).ok();
    match err {
        Some(e) if e.error == "invalid_grant" => Err(Error::revoked(
            "Your Spotify session expired. Please log in again.",
        )),
        Some(e) => Err(Error::auth(format!(
            "Token request failed: {}{}",
            e.error,
            e.error_description
                .map(|d| format!(" ({d})"))
                .unwrap_or_default()
        ))),
        None => Err(Error::Http {
            status: status.as_u16(),
            message: "token endpoint error".into(),
        }),
    }
}

/// Exchange the authorization code for tokens.
pub async fn exchange_code(
    http: &reqwest::Client,
    accounts_base: &str,
    client_id: &str,
    code: &Secret,
    redirect_uri: &str,
    verifier: &Secret,
) -> Result<TokenSet> {
    token_request(
        http,
        accounts_base,
        &[
            ("grant_type", "authorization_code"),
            ("code", code.expose()),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
            ("code_verifier", verifier.expose()),
        ],
        None,
    )
    .await
}

/// Refresh the access token. Spotify may or may not rotate the refresh token;
/// the old one is kept when no new one is returned.
pub async fn refresh(
    http: &reqwest::Client,
    accounts_base: &str,
    client_id: &str,
    refresh_token: &Secret,
) -> Result<TokenSet> {
    token_request(
        http,
        accounts_base,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.expose()),
            ("client_id", client_id),
        ],
        Some(refresh_token),
    )
    .await
}

/// Open a URL in the default browser without pulling in another crate.
pub fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "linux")]
    let mut cmd = std::process::Command::new("xdg-open");
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| Error::Io(format!("could not open a browser: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc7636_appendix_b_vector() {
        // Test vector from RFC 7636, Appendix B.
        let c = challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert_eq!(c, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn generated_verifier_is_valid() {
        let p = Pkce::generate();
        let v = p.verifier.expose();
        assert_eq!(v.len(), 64);
        assert!(v.bytes().all(|b| UNRESERVED.contains(&b)));
        assert_eq!(p.challenge, challenge_for(v));
        assert_ne!(Pkce::generate().verifier, p.verifier);
        assert_ne!(random_state(), random_state());
    }

    #[test]
    fn authorize_url_has_required_params() {
        let u = authorize_url(
            "https://accounts.spotify.com",
            "cid",
            "http://127.0.0.1:8898/login",
            "chal",
            "st",
            &["streaming", "user-library-read"],
        )
        .unwrap();
        let parsed = url::Url::parse(&u).unwrap();
        let q: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(parsed.path(), "/authorize");
        assert_eq!(q["client_id"], "cid");
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["code_challenge"], "chal");
        assert_eq!(q["state"], "st");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:8898/login");
        assert_eq!(q["scope"], "streaming user-library-read");
    }

    #[test]
    fn callback_parsing() {
        let code = |c: Callback| match c {
            Callback::Code(s) => Some(s.expose().to_owned()),
            _ => None,
        };
        let ok = parse_callback("GET /login?code=abc&state=s1 HTTP/1.1", "s1");
        assert_eq!(code(ok).as_deref(), Some("abc"));
        assert!(
            matches!(parse_callback("GET /favicon.ico HTTP/1.1", "s1"), Callback::Ignore(s) if s.starts_with("404"))
        );
        // Wrong, missing state or malformed requests are ignored, not fatal.
        for line in [
            "GET /login?code=abc&state=evil HTTP/1.1",
            "GET /login?code=abc HTTP/1.1",
            "GET /login?error=access_denied HTTP/1.1",
            "POST /login HTTP/1.1",
            "garbage",
            "",
            "GET /login?state=s1 HTTP/1.1",
        ] {
            assert!(
                matches!(parse_callback(line, "s1"), Callback::Ignore(_)),
                "{line}"
            );
        }
        match parse_callback("GET /login?error=access_denied&state=s1 HTTP/1.1", "s1") {
            Callback::Failed(e) => assert!(e.user_message().contains("cancelled")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn token_set_roundtrip_and_expiry() {
        let t = TokenSet {
            access_token: Secret::new("a"),
            refresh_token: Some(Secret::new("r")),
            expires_at: 1000,
            scope: "streaming".into(),
        };
        let back = TokenSet::from_secret(&t.to_secret().unwrap()).unwrap();
        assert_eq!(back, t);
        assert!(!t.is_expired(900));
        assert!(t.is_expired(941));
        assert!(t.is_expired(u64::MAX), "no overflow");
        assert!(format!("{t:?}").contains("[REDACTED]"));
        assert!(!format!("{t:?}").contains("\"a\""));
    }

    #[test]
    fn refresh_keeps_previous_refresh_token() {
        let r = TokenResponse {
            access_token: "new".into(),
            refresh_token: None,
            expires_in: 3600,
            scope: String::new(),
        };
        let prev = Secret::new("old-refresh");
        let t = TokenSet::from_response(r, Some(&prev), 10);
        assert_eq!(t.refresh_token.unwrap().expose(), "old-refresh");
        assert_eq!(t.expires_at, 3610);
    }

    #[tokio::test]
    async fn loopback_listener_receives_code() {
        let listener = bind_callback(0).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(wait_for_callback(
            listener,
            "xyz".into(),
            Duration::from_secs(20),
        ));
        // A silent client, a stray request and a wrong state must not abort.
        let _silent = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        for bad in [
            "GET /favicon.ico HTTP/1.1\r\n\r\n",
            "GET /login?code=x&state=evil HTTP/1.1\r\n\r\n",
        ] {
            let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            s.write_all(bad.as_bytes()).await.unwrap();
            let mut resp = String::new();
            s.read_to_string(&mut resp).await.unwrap();
            assert!(resp.starts_with("HTTP/1.1 4"), "{resp}");
        }
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        s.write_all(b"GET /login?code=the-code&state=xyz HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 200"));
        let code = server.await.unwrap().unwrap();
        assert_eq!(code.expose(), "the-code");
    }
}
