//! MPRIS against a real (private) D-Bus session bus.
//!
//! CI runs `dbus-run-session -- cargo test`, so DBUS_SESSION_BUS_ADDRESS
//! points at a throwaway daemon. Without a bus the test is skipped.
#![cfg(target_os = "linux")]

use std::time::Duration;

use ferrotune::net::mpris::{
    MediaCommand, MediaLoop, MediaSnapshot, MediaStatus, MediaTrack, serve,
};
use mpris_server::zbus::{self, zvariant::OwnedValue};
use tokio::sync::mpsc;

const IFACE: &str = "org.mpris.MediaPlayer2.Player";

async fn get(conn: &zbus::Connection, bus: &str, iface: &str, prop: &str) -> OwnedValue {
    let proxy = zbus::fdo::PropertiesProxy::builder(conn)
        .destination(bus.to_owned())
        .unwrap()
        .path("/org/mpris/MediaPlayer2")
        .unwrap()
        .build()
        .await
        .unwrap();
    proxy
        .get(zbus::names::InterfaceName::try_from(iface).unwrap(), prop)
        .await
        .unwrap()
}

#[tokio::test]
async fn mpris_exposes_state_and_forwards_commands() {
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
        eprintln!("skipping: no session bus (run under dbus-run-session)");
        return;
    }
    let suffix = format!("ferrotune_test_{}", std::process::id());
    let bus = format!("org.mpris.MediaPlayer2.{suffix}");
    let (snap_tx, snap_rx) = mpsc::unbounded_channel();
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel();
    let s = suffix.clone();
    tokio::spawn(async move { serve(&s, snap_rx, cmd_tx).await });
    snap_tx
        .send(MediaSnapshot {
            status: MediaStatus::Playing,
            track: Some(MediaTrack {
                id: "abc123".into(),
                title: "Glass Satellites".into(),
                artists: vec!["Nova Harbor".into()],
                album: "Midnight Cartography".into(),
                length_ms: 200_000,
                art_url: None,
            }),
            position_ms: 1000,
            volume: 0.5,
            shuffle: true,
            loop_status: MediaLoop::Playlist,
            can_next: true,
            can_previous: true,
            seek_seq: 0,
        })
        .unwrap();

    let conn = zbus::Connection::session().await.unwrap();
    // Wait for the name to appear.
    let dbus = zbus::fdo::DBusProxy::new(&conn).await.unwrap();
    for _ in 0..50 {
        if dbus
            .name_has_owner(zbus::names::BusName::try_from(bus.as_str()).unwrap())
            .await
            .unwrap_or(false)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    let identity: String = get(&conn, &bus, "org.mpris.MediaPlayer2", "Identity")
        .await
        .try_into()
        .unwrap();
    assert_eq!(identity, "Ferrotune");
    let desktop: String = get(&conn, &bus, "org.mpris.MediaPlayer2", "DesktopEntry")
        .await
        .try_into()
        .unwrap();
    assert_eq!(desktop, "dev.ferro.ferrotune");
    let status: String = get(&conn, &bus, IFACE, "PlaybackStatus")
        .await
        .try_into()
        .unwrap();
    assert_eq!(status, "Playing");
    let looping: String = get(&conn, &bus, IFACE, "LoopStatus")
        .await
        .try_into()
        .unwrap();
    assert_eq!(looping, "Playlist");
    let shuffle: bool = get(&conn, &bus, IFACE, "Shuffle").await.try_into().unwrap();
    assert!(shuffle);
    let volume: f64 = get(&conn, &bus, IFACE, "Volume").await.try_into().unwrap();
    assert!((volume - 0.5).abs() < 1e-9);
    let meta: std::collections::HashMap<String, OwnedValue> = get(&conn, &bus, IFACE, "Metadata")
        .await
        .try_into()
        .unwrap();
    let title: String = meta["xesam:title"].try_clone().unwrap().try_into().unwrap();
    assert_eq!(title, "Glass Satellites");
    let len: i64 = meta["mpris:length"]
        .try_clone()
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(len, 200_000_000);

    // Media keys: PlayPause and Next arrive as commands.
    let player = zbus::Proxy::new(&conn, bus.as_str(), "/org/mpris/MediaPlayer2", IFACE)
        .await
        .unwrap();
    player.call_method("PlayPause", &()).await.unwrap();
    player.call_method("Next", &()).await.unwrap();
    player.set_property("Volume", 0.25f64).await.unwrap();
    let mut got = Vec::new();
    for _ in 0..3 {
        got.push(
            tokio::time::timeout(Duration::from_secs(2), cmd_rx.recv())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    assert_eq!(
        got,
        vec![
            MediaCommand::PlayPause,
            MediaCommand::Next,
            MediaCommand::SetVolume(0.25)
        ]
    );
}
