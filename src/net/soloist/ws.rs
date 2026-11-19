//! Client for Soloist's local WebSocket API.
//!
//! Soloist is started with `--ws 127.0.0.1:0`; once its server listens it
//! writes the bind address and the real port into `ws.addr` / `ws.port` in
//! its data directory (removed on shutdown). [`discover`] reads them; only
//! loopback endpoints are accepted, since the API has no authentication.

use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use super::protocol::{self, Command, Event, MAX_FRAME};

/// Parse the contents of `ws.addr` and `ws.port`. Loopback only.
pub fn parse_endpoint(addr: &str, port: &str) -> Option<SocketAddr> {
    let ip: IpAddr = addr.trim().parse().ok()?;
    let port: u16 = port.trim().parse().ok()?;
    (ip.is_loopback() && port != 0).then_some(SocketAddr::new(ip, port))
}

/// Read the endpoint files once.
pub fn read_endpoint(data_dir: &Path) -> Option<SocketAddr> {
    let read = |f: &str| {
        let s = std::fs::read_to_string(data_dir.join(f)).ok()?;
        (s.len() < 128).then_some(s)
    };
    // `--ws 127.0.0.1:0` writes the OS-assigned port to `ws.port`;
    // `ws.addr` may be absent, and we always bind loopback.
    let addr = read("ws.addr").unwrap_or_else(|| "127.0.0.1".to_owned());
    parse_endpoint(&addr, &read("ws.port")?)
}

/// Remove endpoint files a crashed instance left behind, so discovery
/// never connects to a stale port.
pub fn clear_endpoint(data_dir: &Path) {
    for f in ["ws.addr", "ws.port"] {
        let _ = std::fs::remove_file(data_dir.join(f));
    }
}

/// Wait for the endpoint files (polling), up to `timeout`.
pub async fn discover(data_dir: &Path, timeout: Duration) -> Option<SocketAddr> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(a) = read_endpoint(data_dir) {
            return Some(a);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME))
        .max_frame_size(Some(MAX_FRAME))
}

/// Connect and pump: commands from `cmds` go out as JSON text frames,
/// parsed events go to `events`. Returns when the connection closes, the
/// command channel closes, or `events` is dropped.
pub async fn run(
    addr: SocketAddr,
    cmds: &mut mpsc::UnboundedReceiver<Command>,
    events: &mpsc::UnboundedSender<Event>,
) -> Result<(), String> {
    let url = format!("ws://{addr}");
    let (ws, _) = tokio_tungstenite::connect_async_with_config(url, Some(config()), true)
        .await
        .map_err(|e| e.to_string())?;
    let (mut tx, mut rx) = ws.split();
    loop {
        tokio::select! {
            cmd = cmds.recv() => {
                let Some(cmd) = cmd else { return Ok(()) };
                tracing::debug!(command = cmd.name(), "soloist command");
                tx.send(Message::text(cmd.to_json().to_string()))
                    .await
                    .map_err(|e| e.to_string())?;
            }
            msg = rx.next() => match msg {
                Some(Ok(Message::Text(t))) => {
                    match protocol::parse_event(t.as_str()) {
                        Some(Event::Error { message }) => {
                            let m: String = message.chars().take(200).collect();
                            tracing::debug!(error = %m, "soloist refused a command");
                            if events.send(Event::Error { message: m }).is_err() {
                                return Ok(());
                            }
                        }
                        Some(ev) => {
                            if events.send(ev).is_err() {
                                return Ok(());
                            }
                        }
                        None => tracing::debug!("unreadable soloist event"),
                    }
                }
                Some(Ok(Message::Close(_))) | None => return Ok(()),
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e.to_string()),
            },
            () = events.closed() => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_files_are_validated() {
        assert_eq!(
            parse_endpoint("127.0.0.1\n", "9090\n"),
            Some("127.0.0.1:9090".parse().unwrap())
        );
        assert_eq!(parse_endpoint("::1", "9"), Some("[::1]:9".parse().unwrap()));
        assert_eq!(parse_endpoint("0.0.0.0", "9090"), None, "not loopback");
        assert_eq!(parse_endpoint("192.168.1.2", "9090"), None);
        assert_eq!(parse_endpoint("127.0.0.1", "0"), None);
        assert_eq!(parse_endpoint("127.0.0.1", "99999"), None);
        assert_eq!(parse_endpoint("localhost", "1"), None);
    }

    #[test]
    fn endpoint_files_on_disk() {
        let dir = std::env::temp_dir().join(format!("ferrotune-ws-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_endpoint(&dir), None);
        std::fs::write(dir.join("ws.addr"), "127.0.0.1").unwrap();
        std::fs::write(dir.join("ws.port"), "4567").unwrap();
        assert_eq!(read_endpoint(&dir), Some("127.0.0.1:4567".parse().unwrap()));
        clear_endpoint(&dir);
        assert_eq!(read_endpoint(&dir), None);
        // Soloist 1.3.10 with `--ws 127.0.0.1:0`: only `ws.port`.
        std::fs::write(dir.join("ws.port"), "40123\n").unwrap();
        assert_eq!(
            read_endpoint(&dir),
            Some("127.0.0.1:40123".parse().unwrap())
        );
        clear_endpoint(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
