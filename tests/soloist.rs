//! Integration tests for the Spotify Soloist engine manager, with no real
//! network and no real Soloist:
//!
//! * a fake CDN (wiremock) serving `.tar.gz` archives that contain a fake
//!   `soloist` shell script;
//! * a fake Soloist WebSocket server (tokio-tungstenite) that emits the
//!   documented events and records the command JSON it receives;
//! * the real supervisor, which installs the fake build, starts it with
//!   the documented arguments, follows its WebSocket API, restarts it after
//!   a crash, updates it after exit code 10, and stops it on shutdown.
//!
//! The fake daemon prints its whole argv (API key included) to stdout, and
//! every log record is captured *unredacted* by the test subscriber, so the
//! test proves the supervisor's own redaction.

#![cfg(unix)]

use std::io::Write as _;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use ferrotune::auth::secrets::Secret;
use ferrotune::net::soloist::install::{self, Fetched, Paths};
use ferrotune::net::soloist::protocol::{Command, Event, RepeatMode, repeat_commands};
use ferrotune::net::soloist::{
    self, Control, EngineStatus, SoloistEvent, SoloistHandle, SoloistKind,
};
use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "sk-test-0123456789abcdef";

// ------------------------------------------------------------- log capture

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Capture {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut v) = self.0.lock() {
            v.extend_from_slice(b);
        }
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn logs() -> &'static Capture {
    static LOGS: OnceLock<Capture> = OnceLock::new();
    LOGS.get_or_init(|| {
        let c = Capture::default();
        let w = c.clone();
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .with_writer(move || w.clone())
            .try_init();
        c
    })
}

fn log_text() -> String {
    String::from_utf8_lossy(&logs().0.lock().unwrap()).into_owned()
}

// --------------------------------------------------------------- archives

fn tar_entry(name: &str, data: &[u8]) -> Vec<u8> {
    let mut h = [0u8; 512];
    h[..name.len()].copy_from_slice(name.as_bytes());
    h[100..108].copy_from_slice(b"0000755\0");
    h[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
    h[136..148].copy_from_slice(b"00000000000\0");
    h[156] = b'0';
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    h[148..156].copy_from_slice(b"        ");
    let sum: u32 = h.iter().map(|b| u32::from(*b)).sum();
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    let mut v = h.to_vec();
    v.extend_from_slice(data);
    v.resize(v.len().div_ceil(512) * 512, 0);
    v
}

fn targz(script: &str) -> Vec<u8> {
    let mut tar = tar_entry("README.txt", b"not the binary");
    tar.extend(tar_entry("soloist", script.as_bytes()));
    tar.extend_from_slice(&[0u8; 1024]);
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&tar).unwrap();
    gz.finish().unwrap()
}

/// A fake `soloist`: answers --version/--help, prints its argv (with the
/// key), writes ws.addr/ws.port pointing at the fake WebSocket server and
/// stays alive. With `expire_once`, the first daemon start exits with 10.
fn fake_soloist(version: &str, built: &str, ws_port: u16, expire_once: bool) -> String {
    let expire = if expire_once {
        r#"if [ ! -e "$DATA/expired-once" ]; then : > "$DATA/expired-once"; echo "build expired"; exit 10; fi"#
    } else {
        ""
    };
    format!(
        r#"#!/bin/sh
case "$1" in
  --version) echo "Spotify Soloist {version} (build abc123)"; echo "built: {built}"; echo "linux x86_64"; exit 0;;
  --help) echo "usage: soloist -n NAME -k KEY [options]"; exit 0;;
esac
echo "fake soloist argv: $*"
echo "env key: ${{SOLOIST_API_KEY:-none}}" >&2
DATA=""
for a in "$@"; do
  if [ "$prev" = "--data-dir" ]; then DATA="$a"; fi
  prev="$a"
done
echo "$*" > "$DATA/argv"
{expire}
echo $$ > "$DATA/fake.pid"
printf 127.0.0.1 > "$DATA/ws.addr"
printf {ws_port} > "$DATA/ws.port"
exec sleep 600
"#
    )
}

fn temp_paths(name: &str) -> Paths {
    let root = std::env::temp_dir().join(format!(
        "ferrotune-soloist-it-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    Paths::new(&root.join("data"), &root.join("cache"))
}

// ------------------------------------------------------- fake WS server

/// Accepts connections; every received text frame goes to `received`,
/// every value sent on the returned channel goes out to the client.
async fn fake_ws_server() -> (
    SocketAddr,
    mpsc::UnboundedReceiver<Value>,
    mpsc::UnboundedSender<String>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (rx_tx, rx) = mpsc::unbounded_channel();
    let (out_tx, out_rx) = mpsc::unbounded_channel::<String>();
    let out_rx = Arc::new(tokio::sync::Mutex::new(out_rx));
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let rx_tx = rx_tx.clone();
            let out_rx = out_rx.clone();
            tokio::spawn(async move {
                let Ok(ws) = tokio_tungstenite::accept_async(tcp).await else {
                    return;
                };
                let (mut tx, mut rx) = ws.split();
                // On connect Soloist sends auth_state (docs).
                let hello = r#"{"type":"auth_state","logged_in":false,"is_active":false,"device_name":"Ferrotune on test"}"#;
                if tx.send(Message::text(hello)).await.is_err() {
                    return;
                }
                let mut out = out_rx.lock().await;
                loop {
                    tokio::select! {
                        m = rx.next() => match m {
                            Some(Ok(Message::Text(t))) => {
                                let v: Value = serde_json::from_str(t.as_str()).unwrap();
                                let _ = rx_tx.send(v);
                            }
                            Some(Ok(_)) => {}
                            _ => return,
                        },
                        s = out.recv() => match s {
                            Some(s) => if tx.send(Message::text(s)).await.is_err() { return },
                            None => return,
                        },
                    }
                }
            });
        }
    });
    (addr, rx, out_tx)
}

// ----------------------------------------------------------------- helpers

type Events = std::pin::Pin<Box<dyn futures::Stream<Item = SoloistEvent> + Send>>;

async fn start(paths: &Paths, url: String) -> (SoloistHandle, Events) {
    let mut events: Events = Box::pin(soloist::events(SoloistKind::Real {
        paths: paths.clone(),
        url: Some(url),
    }));
    let Some(SoloistEvent::Ready(h)) = events.next().await else {
        panic!("no handle");
    };
    (h, events)
}

/// Wait for an event matching `pred` (other events are skipped).
async fn until(
    events: &mut Events,
    secs: u64,
    pred: impl Fn(&SoloistEvent) -> bool,
) -> SoloistEvent {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let next = tokio::time::timeout_at(deadline, events.next())
            .await
            .unwrap_or_else(|_| panic!("timed out; logs:\n{}", log_text()))
            .expect("stream ended");
        if pred(&next) {
            return next;
        }
    }
}

fn status_is(s: EngineStatus) -> impl Fn(&SoloistEvent) -> bool {
    move |e| matches!(e, SoloistEvent::Status(x) if *x == s)
}

async fn recv_json(rx: &mut mpsc::UnboundedReceiver<Value>) -> Value {
    tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("no command received")
        .expect("server closed")
}

fn configure(device: &str) -> Control {
    Control::Configure {
        key: Some(Secret::new(KEY)),
        device_name: device.into(),
        pipewire_device: None,
    }
}

fn pid_alive(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
        && !std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .unwrap_or_default()
            .contains(") Z ")
}

fn read_pid(data: &Path) -> i32 {
    std::fs::read_to_string(data.join("fake.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

async fn wait_file(p: &Path) {
    for _ in 0..100 {
        if p.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{} never appeared", p.display());
}

// ------------------------------------------------------------------- tests

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_update_and_caps_against_a_fake_cdn() {
    let server = MockServer::start().await;
    let paths = temp_paths("cdn");
    let url = format!("{}/soloist_release_x86_64.tar.gz", server.uri());
    let v1 = targz(&fake_soloist("1.0.0", "2026-10-01T00:00:00Z", 1, false));
    Mock::given(method("GET"))
        .and(path("/soloist_release_x86_64.tar.gz"))
        .and(header("if-none-match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/soloist_release_x86_64.tar.gz"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"v1\"")
                .insert_header("last-modified", "Thu, 01 Oct 2026 00:00:00 GMT")
                .set_body_bytes(v1),
        )
        .with_priority(5)
        .mount(&server)
        .await;
    let http = soloist::http_client();

    let Fetched::Installed(meta) = install::fetch(&http, &url, &paths, None).await.unwrap() else {
        panic!("not installed");
    };
    assert_eq!(meta.version.as_deref(), Some("1.0.0"));
    assert_eq!(
        ferrotune::net::soloist::archive::format_date(meta.expires_unix()),
        "2026-12-30"
    );
    assert_eq!(meta.etag.as_deref(), Some("\"v1\""));
    assert!(!meta.key_from_env);
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(paths.binary())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
    assert_eq!(install::InstallMeta::load(&paths), Some(meta.clone()));
    assert!(!paths.root.join("download.part").exists());

    // Conditional GET: nothing new.
    assert_eq!(
        install::fetch(&http, &url, &paths, Some(&meta))
            .await
            .unwrap(),
        Fetched::NotModified
    );

    // An older build on the CDN never replaces a newer installed one.
    let newer = install::InstallMeta {
        built_unix: Some(meta.built_unix.unwrap() + 86_400),
        etag: Some("\"other\"".into()),
        ..meta.clone()
    };
    assert_eq!(
        install::fetch(&http, &url, &paths, Some(&newer))
            .await
            .unwrap(),
        Fetched::Older
    );

    // Not a gzip archive / missing binary / server error.
    let bad = MockServer::start().await;
    Mock::given(path("/zip"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"PK\x03\x04".to_vec()))
        .mount(&bad)
        .await;
    Mock::given(path("/nobin"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes({
            let mut tar = tar_entry("other", b"x");
            tar.extend_from_slice(&[0u8; 1024]);
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            gz.write_all(&tar).unwrap();
            gz.finish().unwrap()
        }))
        .mount(&bad)
        .await;
    Mock::given(path("/gone"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&bad)
        .await;
    let other = temp_paths("cdn-bad");
    for p in ["/zip", "/nobin", "/gone"] {
        let e = install::fetch(&http, &format!("{}{p}", bad.uri()), &other, None).await;
        assert!(e.is_err(), "{p} must fail");
        assert!(!other.binary().exists(), "{p} must not install anything");
    }
    let _ = std::fs::remove_dir_all(paths.root.parent().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supervisor_runs_controls_and_stops_the_daemon() {
    logs();
    let (ws_addr, mut received, to_client) = fake_ws_server().await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_bytes(targz(&fake_soloist(
                "1.0.0",
                "2026-11-01T00:00:00Z",
                ws_addr.port(),
                false,
            ))),
        )
        .mount(&server)
        .await;
    let paths = temp_paths("run");
    let (h, mut ev) = start(&paths, format!("{}/a.tar.gz", server.uri())).await;

    // Without a key: NeedsKey, nothing is downloaded.
    h.send(Control::Configure {
        key: None,
        device_name: "x".into(),
        pipewire_device: None,
    });
    until(&mut ev, 5, status_is(EngineStatus::NeedsKey)).await;
    assert!(!paths.binary().exists());

    h.send(configure("Ferrotune on test"));
    until(&mut ev, 10, status_is(EngineStatus::Installing)).await;
    let SoloistEvent::Build(b) = until(&mut ev, 10, |e| matches!(e, SoloistEvent::Build(_))).await
    else {
        unreachable!()
    };
    assert_eq!(b.version.as_deref(), Some("1.0.0"));
    until(&mut ev, 10, status_is(EngineStatus::Starting)).await;
    until(&mut ev, 10, |e| matches!(e, SoloistEvent::Connected(true))).await;
    // auth_state logged_in=false on connect: the device must be paired.
    until(&mut ev, 10, status_is(EngineStatus::NeedsPairing)).await;

    // The documented argv; the key is in argv because --help does not
    // mention SOLOIST_API_KEY.
    let data = paths.data();
    let argv = std::fs::read_to_string(data.join("argv")).unwrap();
    assert_eq!(
        argv.trim(),
        format!(
            "--device-name Ferrotune on test --data-dir {} --cache-dir {} --ws 127.0.0.1:0 --api-key {KEY}",
            data.display(),
            paths.cache.display()
        )
    );

    // Pairing done in the Spotify app: Soloist sends a new auth_state.
    to_client
        .send(r#"{"type":"auth_state","logged_in":true,"is_active":true,"device_name":"Ferrotune on test"}"#.into())
        .unwrap();
    until(&mut ev, 5, status_is(EngineStatus::Ready)).await;

    // Commands reach Soloist exactly as documented.
    h.command(Command::Pause);
    assert_eq!(
        recv_json(&mut received).await,
        serde_json::json!({"type": "command", "command": "pause"})
    );
    h.command(Command::Play(Some(
        "spotify:album:4aawyAB9vmqN3uQ7FjRGTy".into(),
    )));
    assert_eq!(
        recv_json(&mut received).await,
        serde_json::json!({"type": "command", "command": "play", "uri": "spotify:album:4aawyAB9vmqN3uQ7FjRGTy"})
    );
    for c in repeat_commands(RepeatMode::Context) {
        h.command(c);
    }
    assert_eq!(
        recv_json(&mut received).await,
        serde_json::json!({"type": "command", "command": "set_repeat_track", "enabled": false})
    );
    assert_eq!(
        recv_json(&mut received).await,
        serde_json::json!({"type": "command", "command": "set_repeat_context", "enabled": true})
    );
    h.command(Command::SetVolume(42));
    assert_eq!(recv_json(&mut received).await["volume"], 42);

    // Events from Soloist come back parsed.
    to_client
        .send(r#"{"type":"volume_changed","volume":42}"#.into())
        .unwrap();
    until(
        &mut ev,
        5,
        |e| matches!(e, SoloistEvent::Ws(Event::VolumeChanged { volume }) if *volume == 42.0),
    )
    .await;

    // Crash: the supervisor restarts it after the backoff.
    let pid = read_pid(&data);
    std::fs::remove_file(data.join("fake.pid")).unwrap();
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
    until(&mut ev, 5, |e| {
        matches!(e, SoloistEvent::Status(EngineStatus::Error(_)))
    })
    .await;
    until(&mut ev, 10, status_is(EngineStatus::Starting)).await;
    wait_file(&data.join("fake.pid")).await;
    let pid2 = read_pid(&data);
    assert_ne!(pid, pid2);
    assert!(pid_alive(pid2));

    // Shutdown terminates the child.
    h.send(Control::Shutdown);
    for _ in 0..60 {
        if !pid_alive(pid2) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!pid_alive(pid2), "soloist must be stopped on shutdown");

    // The key never reached a log line, although the fake daemon printed it.
    let text = log_text();
    assert!(
        text.contains("fake soloist argv"),
        "daemon output is logged"
    );
    assert!(!text.contains(KEY), "API key leaked into the logs:\n{text}");
    let _ = std::fs::remove_dir_all(paths.root.parent().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_build_is_updated_and_restarted() {
    logs();
    let (ws_addr, _received, _to_client) = fake_ws_server().await;
    let server = MockServer::start().await;
    // First download: a build that exits with code 10 once started.
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"old\"")
                .set_body_bytes(targz(&fake_soloist(
                    "1.0.0",
                    "2026-08-01T00:00:00Z",
                    ws_addr.port(),
                    true,
                ))),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    // Then a newer build.
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"new\"")
                .set_body_bytes(targz(&fake_soloist(
                    "1.1.0",
                    "2026-11-10T00:00:00Z",
                    ws_addr.port(),
                    false,
                ))),
        )
        .with_priority(2)
        .mount(&server)
        .await;
    let paths = temp_paths("expired");
    let (h, mut ev) = start(&paths, format!("{}/a.tar.gz", server.uri())).await;
    h.send(configure("Ferrotune on test"));
    until(&mut ev, 10, status_is(EngineStatus::Starting)).await;
    // Exit 10 -> update -> restart with the new build.
    until(&mut ev, 10, status_is(EngineStatus::Updating)).await;
    let SoloistEvent::Build(b) = until(&mut ev, 10, |e| matches!(e, SoloistEvent::Build(_))).await
    else {
        unreachable!()
    };
    assert_eq!(b.version.as_deref(), Some("1.1.0"));
    until(&mut ev, 10, status_is(EngineStatus::Starting)).await;
    until(&mut ev, 10, status_is(EngineStatus::NeedsPairing)).await;
    let meta = install::InstallMeta::load(&paths).unwrap();
    assert_eq!(meta.etag.as_deref(), Some("\"new\""));
    h.send(Control::Shutdown);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = std::fs::remove_dir_all(paths.root.parent().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pair_mode_runs_without_websocket_and_restarts() {
    logs();
    let (ws_addr, _received, _to_client) = fake_ws_server().await;
    let server = MockServer::start().await;
    // --pair: the fake exits 0 at once (as if the app had paired).
    let script = fake_soloist("1.0.0", "2026-11-01T00:00:00Z", ws_addr.port(), false).replace(
        "echo \"$*\" > \"$DATA/argv\"",
        "echo \"$*\" > \"$DATA/argv\"\ncase \"$*\" in *--pair*) echo \"$*\" > \"$DATA/pair-argv\"; exit 0;; esac",
    );
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(targz(&script)))
        .mount(&server)
        .await;
    let paths = temp_paths("pair");
    let (h, mut ev) = start(&paths, format!("{}/a.tar.gz", server.uri())).await;
    h.send(configure("Desk"));
    until(&mut ev, 10, status_is(EngineStatus::NeedsPairing)).await;
    h.send(Control::Pair);
    until(&mut ev, 10, status_is(EngineStatus::Pairing)).await;
    until(&mut ev, 10, status_is(EngineStatus::NeedsPairing)).await;
    let pair_argv = std::fs::read_to_string(paths.data().join("pair-argv")).unwrap();
    assert!(pair_argv.contains("--pair"));
    assert!(!pair_argv.contains("--ws"), "{pair_argv}");
    h.send(Control::Shutdown);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = std::fs::remove_dir_all(paths.root.parent().unwrap());
}
