//! The StatusNotifierItem service on a private session bus.
//!
//! Needs a bus: run as `dbus-run-session -- cargo test --test tray_sni`.
//! Without `DBUS_SESSION_BUS_ADDRESS` the test skips itself, so a plain
//! `cargo test` stays green on machines without a session bus.

#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferrotune::tray::{self, Command, Event, TrayModel};
use futures::StreamExt;
use zbus::zvariant::{OwnedValue, Value};

const WAIT: Duration = Duration::from_secs(10);

/// Minimal `org.kde.StatusNotifierWatcher`.
struct Watcher {
    items: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    async fn register_status_notifier_item(&self, service: &str) {
        if let Ok(mut items) = self.items.lock() {
            items.push(service.to_owned());
        }
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.items.lock().map(|i| i.clone()).unwrap_or_default()
    }
}

async fn next(stream: &mut (impl futures::Stream<Item = Event> + Unpin)) -> Event {
    tokio::time::timeout(WAIT, stream.next())
        .await
        .expect("tray event within timeout")
        .expect("tray stream open")
}

type Pixmaps = Vec<(i32, i32, Vec<u8>)>;
type ToolTip = (String, Pixmaps, String, String);
type Layout = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

fn label(props: &HashMap<String, OwnedValue>) -> Option<String> {
    props
        .get("label")
        .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
}

#[tokio::test(flavor = "multi_thread")]
async fn tray_registers_updates_and_reports_clicks() {
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
        eprintln!("skipped: no D-Bus session bus (use dbus-run-session)");
        return;
    }

    // 1. No watcher on the bus: the tray reports "unavailable", so the app
    //    keeps "closing the window quits".
    let mut stream = Box::pin(tray::events());
    assert!(matches!(next(&mut stream).await, Event::Unavailable));
    drop(stream);

    // 2. A watcher appears: the item registers.
    let items = Arc::new(Mutex::new(Vec::new()));
    let watcher_conn = zbus::connection::Builder::session()
        .unwrap()
        .name("org.kde.StatusNotifierWatcher")
        .unwrap()
        .serve_at(
            "/StatusNotifierWatcher",
            Watcher {
                items: items.clone(),
            },
        )
        .unwrap()
        .build()
        .await
        .unwrap();
    let mut stream = Box::pin(tray::events());
    let Event::Ready(control) = next(&mut stream).await else {
        panic!("expected Ready");
    };
    let service = items.lock().unwrap().first().cloned().expect("registered");
    assert!(service.starts_with("org.kde.StatusNotifierItem-"));

    // 3. The exported item has the right properties.
    let client = zbus::Connection::session().await.unwrap();
    // SNI signals (NewStatus, ...) do not carry property values, so the
    // proxy must not cache them.
    let item = zbus::proxy::Builder::<zbus::Proxy>::new(&client)
        .destination(service.as_str())
        .unwrap()
        .path("/StatusNotifierItem")
        .unwrap()
        .interface("org.kde.StatusNotifierItem")
        .unwrap()
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .unwrap();
    let id: String = item.get_property("Id").await.unwrap();
    assert_eq!(id, "dev.ferro.ferrotune");
    let category: String = item.get_property("Category").await.unwrap();
    assert_eq!(category, "ApplicationStatus");
    let status: String = item.get_property("Status").await.unwrap();
    assert_eq!(status, "Active");
    let pixmaps: Pixmaps = item.get_property("IconPixmap").await.unwrap();
    assert_eq!(pixmaps.len(), 3);
    assert_eq!(
        pixmaps[0].2.len(),
        (pixmaps[0].0 * pixmaps[0].1 * 4) as usize
    );
    let tip: ToolTip = item.get_property("ToolTip").await.unwrap();
    assert_eq!(tip.2, "Ferrotune", "idle tooltip");

    // 4. A track plays: the tooltip names it; the status stays Active (a
    //    music player has no attention state).
    control.publish(TrayModel {
        track: Some("Nova \u{2014} Midnight".into()),
        playing: true,
        ..Default::default()
    });
    let mut title = String::new();
    for _ in 0..50 {
        let tip: ToolTip = item.get_property("ToolTip").await.unwrap();
        title = tip.2;
        if title != "Ferrotune" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(title, "Nova \u{2014} Midnight");
    let status: String = item.get_property("Status").await.unwrap();
    assert_eq!(status, "Active");

    // 5. Left click toggles the window.
    item.call_method("Activate", &(0i32, 0i32)).await.unwrap();
    assert!(matches!(
        next(&mut stream).await,
        Event::Command(Command::Toggle)
    ));

    // 6. The menu (dbusmenu) has the expected entries and they send commands.
    let menu = zbus::Proxy::new(
        &client,
        service.as_str(),
        "/MenuBar",
        "com.canonical.dbusmenu",
    )
    .await
    .unwrap();
    let (_rev, (_root, _props, children)): (u32, Layout) = menu
        .call("GetLayout", &(0i32, -1i32, Vec::<String>::new()))
        .await
        .unwrap();
    let mut entries: Vec<(i32, String, Vec<OwnedValue>)> = Vec::new();
    for c in children {
        let (cid, props, sub): Layout = c.try_into().unwrap();
        if let Some(l) = label(&props) {
            entries.push((cid, l, sub));
        }
    }
    let names: Vec<&str> = entries.iter().map(|(_, l, _)| l.as_str()).collect();
    assert_eq!(
        names,
        [
            "Hide Ferrotune",
            "Pause",
            "Next",
            "Previous",
            "Start minimized to tray",
            "Quit Ferrotune"
        ]
    );
    let click = |id: i32| {
        let menu = menu.clone();
        async move {
            let _: () = menu
                .call("Event", &(id, "clicked", Value::from(0i32), 0u32))
                .await
                .unwrap();
        }
    };
    let id_of = |name: &str| entries.iter().find(|(_, l, _)| l == name).unwrap().0;
    for (label, command) in [
        ("Pause", Command::PlayPause),
        ("Next", Command::Next),
        ("Previous", Command::Previous),
        ("Start minimized to tray", Command::ToggleStartMinimized),
        ("Quit Ferrotune", Command::Quit),
    ] {
        click(id_of(label)).await;
        match next(&mut stream).await {
            Event::Command(c) => assert_eq!(c, command),
            other => panic!("unexpected {other:?}"),
        }
    }

    // 6b. Scroll on the icon changes the volume.
    item.call_method("Scroll", &(1i32, "vertical"))
        .await
        .unwrap();
    assert!(matches!(
        next(&mut stream).await,
        Event::Command(Command::VolumeStep(d)) if d > 0.0
    ));
    item.call_method("Scroll", &(-1i32, "vertical"))
        .await
        .unwrap();
    assert!(matches!(
        next(&mut stream).await,
        Event::Command(Command::VolumeStep(d)) if d < 0.0
    ));

    // 7. The panel goes away: the app is told, so a hidden window returns.
    watcher_conn.graceful_shutdown().await;
    assert!(matches!(next(&mut stream).await, Event::Unavailable));
}
