//! Integration tests: the internal-API backends (spclient protobuf,
//! collection2v2, pathfinder GraphQL) against a local wiremock server.
//!
//! Protobuf responses are built from librespot-protocol messages (the same
//! types the real endpoints return); JSON responses are fixtures under
//! `tests/fixtures/`. No real network, no real account.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use ferrotune::auth::secrets::Secret;
use ferrotune::error::Error;
use ferrotune::net::backend::Backend;
use ferrotune::net::client::{InternalClient, SessionAuth, http_client};
use ferrotune::net::model::Card;
use ferrotune::net::pathfinder::Queries;
use ferrotune::net::proto::{self, CollectionItem, DeltaResponse, PageResponse};
use ferrotune::net::tokens::{AccessToken, TokenSource};
use ferrotune::net::types::gid_to_id;
use ferrotune::store::Store;
use futures::future::BoxFuture;
use librespot_protocol::entity_extension_data::{EntityExtensionData, EntityExtensionDataHeader};
use librespot_protocol::extended_metadata::{
    BatchedEntityRequest, BatchedExtensionResponse, EntityExtensionDataArray,
};
use librespot_protocol::metadata;
use librespot_protocol::playlist4_external::{
    Item, ItemAttributes, ListAttributes, ListItems, MetaItem, SelectedListContent,
};
use protobuf::well_known_types::any::Any;
use protobuf::{Message, MessageField};
use wiremock::matchers::{header, method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

// ------------------------------------------------------------ fake session

struct FakeAuth {
    fetches: AtomicU32,
}

impl TokenSource for FakeAuth {
    fn fetch(&self) -> BoxFuture<'_, ferrotune::error::Result<AccessToken>> {
        Box::pin(async move {
            let n = self.fetches.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(AccessToken {
                secret: Secret::new(format!("session-token-{n}")),
                expires_at: Instant::now() + Duration::from_secs(3600),
            })
        })
    }
}

impl SessionAuth for FakeAuth {
    fn client_token(&self) -> BoxFuture<'_, ferrotune::error::Result<Option<String>>> {
        Box::pin(async { Ok(Some("client-token-1".into())) })
    }
    fn spclient_base(&self) -> BoxFuture<'_, ferrotune::error::Result<String>> {
        Box::pin(async { Err(Error::Net("tests use an override".into())) })
    }
    fn username(&self) -> String {
        "ferro_user".into()
    }
    fn country(&self) -> String {
        "SE".into()
    }
    fn product(&self) -> Option<String> {
        Some("premium".into())
    }
}

struct Env {
    server: MockServer,
    auth: Arc<FakeAuth>,
    client: Arc<InternalClient>,
    backend: Backend,
    store: Store,
}

async fn env() -> Env {
    let server = MockServer::start().await;
    let auth = Arc::new(FakeAuth {
        fetches: AtomicU32::new(0),
    });
    let store = Store::in_memory().unwrap();
    let client = Arc::new(InternalClient::new(
        http_client(),
        auth.clone(),
        Some(server.uri()),
        server.uri(),
        Some(store.clone()),
    ));
    let backend = Backend::internal(
        client.clone(),
        Queries::default(),
        None,
        Some(store.clone()),
    );
    Env {
        server,
        auth,
        client,
        backend,
        store,
    }
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

// ------------------------------------------------------------ catalogue

fn gid(n: u8) -> Vec<u8> {
    let mut g = vec![n; 16];
    g[0] = 0x10; // keep the base62 id 22 chars long
    g
}

fn id(n: u8) -> String {
    gid_to_id(&gid(n)).unwrap()
}

fn uri(kind: &str, n: u8) -> String {
    format!("spotify:{kind}:{}", id(n))
}

fn image(file: u8) -> metadata::Image {
    let mut i = metadata::Image::new();
    i.set_file_id(vec![file; 20]);
    i.set_size(metadata::image::Size::DEFAULT);
    i
}

fn artist_msg(n: u8, name: &str) -> metadata::Artist {
    let mut a = metadata::Artist::new();
    a.set_gid(gid(n));
    a.set_name(name.into());
    a
}

fn album_msg(n: u8, tracks: &[u8]) -> metadata::Album {
    let mut a = metadata::Album::new();
    a.set_gid(gid(n));
    a.set_name(format!("Album {n}"));
    a.set_type(metadata::album::Type::ALBUM);
    a.artist.push(artist_msg(200, "Nova Harbor"));
    a.cover.push(image(n));
    let mut d = metadata::Date::new();
    d.set_year(2021);
    d.set_month(3);
    d.set_day(9);
    a.date = MessageField::some(d);
    let mut disc = metadata::Disc::new();
    for t in tracks {
        let mut tr = metadata::Track::new();
        tr.set_gid(gid(*t));
        disc.track.push(tr);
    }
    a.disc.push(disc);
    a
}

fn track_msg(n: u8, album: u8) -> metadata::Track {
    let mut t = metadata::Track::new();
    t.set_gid(gid(n));
    t.set_name(format!("Track {n}"));
    t.set_duration(180_000 + i32::from(n));
    t.artist.push(artist_msg(200, "Nova Harbor"));
    let mut al = metadata::Album::new();
    al.set_gid(gid(album));
    al.set_name(format!("Album {album}"));
    al.cover.push(image(album));
    t.album = MessageField::some(al);
    t
}

/// Answers extended-metadata requests from an in-memory catalogue, like the
/// real endpoint: one `EntityExtensionData` per requested URI it knows.
struct Metadata {
    entities: HashMap<String, Vec<u8>>,
    batches: Arc<AtomicU32>,
}

impl Respond for Metadata {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        self.batches.fetch_add(1, Ordering::SeqCst);
        let Ok(r) = BatchedEntityRequest::parse_from_bytes(&req.body) else {
            return ResponseTemplate::new(400);
        };
        let mut arr = EntityExtensionDataArray::new();
        for e in &r.entity_request {
            if let Some(bytes) = self.entities.get(&e.entity_uri) {
                let mut header = EntityExtensionDataHeader::new();
                header.status_code = 200;
                let mut any = Any::new();
                any.value = bytes.clone();
                let mut data = EntityExtensionData::new();
                data.entity_uri = e.entity_uri.clone();
                data.header = MessageField::some(header);
                data.extension_data = MessageField::some(any);
                arr.extension_data.push(data);
            }
        }
        let mut resp = BatchedExtensionResponse::new();
        resp.extended_metadata.push(arr);
        ResponseTemplate::new(200).set_body_bytes(resp.write_to_bytes().unwrap())
    }
}

fn catalogue() -> HashMap<String, Vec<u8>> {
    let mut m = HashMap::new();
    for t in 1..=12u8 {
        m.insert(uri("track", t), track_msg(t, 100).write_to_bytes().unwrap());
    }
    m.insert(
        uri("album", 100),
        album_msg(100, &[1, 2, 3]).write_to_bytes().unwrap(),
    );
    m.insert(
        uri("album", 101),
        album_msg(101, &[4]).write_to_bytes().unwrap(),
    );
    let mut ar = artist_msg(200, "Nova Harbor");
    ar.portrait.push(image(9));
    let mut top = metadata::TopTracks::new();
    top.set_country("SE".into());
    for t in [5u8, 6] {
        let mut tr = metadata::Track::new();
        tr.set_gid(gid(t));
        top.track.push(tr);
    }
    ar.top_track.push(top);
    let mut g = metadata::AlbumGroup::new();
    let mut al = metadata::Album::new();
    al.set_gid(gid(100));
    g.album.push(al);
    ar.album_group.push(g);
    m.insert(uri("artist", 200), ar.write_to_bytes().unwrap());
    m
}

async fn mount_metadata(server: &MockServer) -> Arc<AtomicU32> {
    let batches = Arc::new(AtomicU32::new(0));
    Mock::given(method("POST"))
        .and(path("/extended-metadata/v0/extended-metadata"))
        .respond_with(Metadata {
            entities: catalogue(),
            batches: batches.clone(),
        })
        .mount(server)
        .await;
    batches
}

fn collection_page(items: &[(&str, i32)], next: &str, sync: &str) -> Vec<u8> {
    proto::encode_page(&PageResponse {
        items: items
            .iter()
            .map(|(u, at)| CollectionItem {
                uri: (*u).into(),
                added_at: *at,
                is_removed: false,
            })
            .collect(),
        next_page_token: next.into(),
        sync_token: sync.into(),
    })
}

fn playlist_content(entries: &[String], name: &str, length: i32) -> Vec<u8> {
    let mut c = SelectedListContent::new();
    let mut attrs = ListAttributes::new();
    attrs.set_name(name.into());
    attrs.set_description("A mix".into());
    attrs.set_picture(vec![0xab; 20]);
    c.attributes = MessageField::some(attrs);
    c.set_length(length);
    c.set_owner_username("ferro_user".into());
    let mut items = ListItems::new();
    items.set_pos(0);
    items.set_truncated(false);
    for e in entries {
        let mut it = Item::new();
        it.set_uri(e.clone());
        let mut a = ItemAttributes::new();
        a.set_timestamp(1_700_000_000_000);
        it.attributes = MessageField::some(a);
        items.items.push(it);
    }
    c.contents = MessageField::some(items);
    c.write_to_bytes().unwrap()
}

// ------------------------------------------------------------------ tests

#[tokio::test]
async fn playlist_uses_protobuf_and_official_client_headers() {
    let e = env().await;
    mount_metadata(&e.server).await;
    let entries = vec![
        uri("track", 1),
        "spotify:episode:podcast".into(),
        "spotify:local:a:b:c:1".into(),
        uri("track", 2),
    ];
    Mock::given(method("GET"))
        .and(path(format!("/playlist/v2/playlist/{}", id(50))))
        .and(query_param("from", "0"))
        .and(query_param("length", "100"))
        .and(header("authorization", "Bearer session-token-1"))
        .and(header("client-token", "client-token-1"))
        .and(header("app-platform", "Linux"))
        .and(header(
            "spotify-app-version",
            librespot_core::version::SPOTIFY_SEMANTIC_VERSION,
        ))
        .and(header("accept", "application/x-protobuf"))
        .respond_with(
            ResponseTemplate::new(200).set_body_bytes(playlist_content(&entries, "Mix", 230)),
        )
        .expect(1)
        .mount(&e.server)
        .await;
    let (pl, page) = e.backend.catalog.playlist(&id(50)).await.unwrap();
    assert_eq!(pl.name, "Mix");
    assert_eq!(pl.total_tracks, 230);
    assert!(pl.images[0].url.starts_with("https://i.scdn.co/image/abab"));
    // Episodes and local files are dropped, but paging counts raw entries.
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.raw_len, 4);
    assert_eq!(page.next_offset(), Some(4));
    assert_eq!(page.items[0].name, "Track 1");
    assert_eq!(page.items[0].added_at.as_deref(), Some("2023-11-14"));
    assert_eq!(page.items[1].artist_line(), "Nova Harbor");

    // A second open is served from the SQLite cache (expect(1) above).
    let (_, again) = e.backend.catalog.playlist(&id(50)).await.unwrap();
    assert_eq!(again.items.len(), 2);
}

#[tokio::test]
async fn album_and_artist_from_extended_metadata() {
    let e = env().await;
    let batches = mount_metadata(&e.server).await;
    let album = e.backend.catalog.album(&id(100)).await.unwrap();
    assert_eq!(album.name, "Album 100");
    assert_eq!(album.release_date, "2021-03-09");
    assert_eq!(album.year(), "2021");
    assert_eq!(
        album
            .tracks
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        ["Track 1", "Track 2", "Track 3"]
    );
    let page = e.backend.catalog.artist_page(&id(200)).await.unwrap();
    assert_eq!(page.artist.name, "Nova Harbor");
    assert_eq!(page.top_tracks.len(), 2);
    assert_eq!(page.albums[0].name, "Album 100");
    assert!(
        batches.load(Ordering::SeqCst) <= 5,
        "metadata is batched, not fetched per item"
    );
}

#[tokio::test]
async fn library_and_liked_songs_from_collection() {
    let e = env().await;
    mount_metadata(&e.server).await;
    // Rootlist with a folder marker that must be skipped.
    let mut c = SelectedListContent::new();
    let mut items = ListItems::new();
    items.set_pos(0);
    items.set_truncated(false);
    for (u, name) in [
        (format!("spotify:playlist:{}", id(60)), "Road trip"),
        ("spotify:start-group:abc:Folder".to_owned(), ""),
        (format!("spotify:playlist:{}", id(61)), "Focus"),
    ] {
        let mut it = Item::new();
        it.set_uri(u);
        items.items.push(it);
        let mut meta = MetaItem::new();
        let mut a = ListAttributes::new();
        a.set_name(name.into());
        meta.attributes = MessageField::some(a);
        meta.set_length(12);
        meta.set_owner_username("ferro_user".into());
        items.meta_items.push(meta);
    }
    c.contents = MessageField::some(items);
    Mock::given(method("GET"))
        .and(path("/playlist/v2/user/ferro_user/rootlist"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(c.write_to_bytes().unwrap()))
        .mount(&e.server)
        .await;
    // "collection" set: two pages (tracks + one album), "artist" set: one.
    let (t1, t2, t3, a1) = (
        uri("track", 1),
        uri("track", 2),
        uri("track", 3),
        uri("album", 101),
    );
    let page2 = collection_page(&[(&t3, 10)], "", "");
    let page1 = collection_page(&[(&t1, 30), (&a1, 25), (&t2, 20)], "tok2", "sync-1");
    let collection = proto::page_request("ferro_user", "collection", "", 300);
    let collection2 = proto::page_request("ferro_user", "collection", "tok2", 300);
    let artists = proto::page_request("ferro_user", "artist", "", 300);
    for (body, resp) in [
        (collection, page1),
        (collection2, page2),
        (
            artists,
            collection_page(&[(&uri("artist", 200), 5)], "", "sync-a"),
        ),
    ] {
        Mock::given(method("POST"))
            .and(path("/collection/v2/paging"))
            .and(header(
                "content-type",
                "application/vnd.collection-v2.spotify.proto",
            ))
            .and(wiremock::matchers::body_bytes(body))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(resp))
            .expect(1)
            .mount(&e.server)
            .await;
    }
    let lib = e.backend.library.library().await.unwrap();
    assert_eq!(
        lib.playlists
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["Road trip", "Focus"]
    );
    assert_eq!(lib.albums.len(), 1);
    assert_eq!(lib.artists[0].name, "Nova Harbor");
    assert_eq!(lib.liked_total, 3);

    // Liked Songs come from the synced index (no second paging round).
    let liked = e.backend.library.saved_tracks(0).await.unwrap();
    assert_eq!(
        liked
            .items
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        ["Track 1", "Track 2", "Track 3"],
        "newest first"
    );
    assert_eq!(liked.total, 3);
    let flags = e
        .backend
        .library
        .contains_saved(vec![id(2), id(9)])
        .await
        .unwrap();
    assert_eq!(flags, [true, false]);
}

#[tokio::test]
async fn like_writes_to_collection_and_updates_the_index() {
    let e = env().await;
    Mock::given(method("POST"))
        .and(path("/collection/v2/paging"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(collection_page(
            &[(&uri("track", 1), 30)],
            "",
            "s1",
        )))
        .mount(&e.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/collection/v2/write"))
        .respond_with(ResponseTemplate::new(200))
        .expect(2)
        .mount(&e.server)
        .await;
    let lib = &e.backend.library;
    assert_eq!(lib.contains_saved(vec![id(7)]).await.unwrap(), [false]);
    lib.set_saved(vec![id(7)], true).await.unwrap();
    assert_eq!(lib.contains_saved(vec![id(7)]).await.unwrap(), [true]);
    lib.set_saved(vec![id(1)], false).await.unwrap();
    assert_eq!(lib.contains_saved(vec![id(1)]).await.unwrap(), [false]);
    let writes: Vec<_> = e
        .server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/collection/v2/write")
        .collect();
    assert!(
        writes[0]
            .body
            .windows(b"collection".len())
            .any(|w| w == b"collection")
    );
}

#[tokio::test]
async fn collection_resyncs_with_a_delta_after_restart() {
    let e = env().await;
    mount_metadata(&e.server).await;
    // A previous run left a synced index in SQLite.
    let first = env_with_store(&e).await;
    Mock::given(method("POST"))
        .and(path("/collection/v2/paging"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(collection_page(
            &[(&uri("track", 1), 30)],
            "",
            "sync-1",
        )))
        .expect(1)
        .mount(&e.server)
        .await;
    assert_eq!(first.library.saved_tracks(0).await.unwrap().total, 1);
    Mock::given(method("POST"))
        .and(path("/collection/v2/delta"))
        .and(wiremock::matchers::body_bytes(proto::delta_request(
            "ferro_user",
            "collection",
            "sync-1",
        )))
        .respond_with(
            ResponseTemplate::new(200).set_body_bytes(proto::encode_delta(&DeltaResponse {
                delta_update_possible: true,
                items: vec![CollectionItem {
                    uri: uri("track", 2),
                    added_at: 40,
                    is_removed: false,
                }],
                sync_token: "sync-2".into(),
            })),
        )
        .expect(1)
        .mount(&e.server)
        .await;
    // New process (fresh backend, same store): delta, not a full sync.
    let second = env_with_store(&e).await;
    let liked = second.library.saved_tracks(0).await.unwrap();
    assert_eq!(liked.total, 2);
    assert_eq!(liked.items[0].name, "Track 2");
}

async fn env_with_store(e: &Env) -> Backend {
    let client = Arc::new(InternalClient::new(
        http_client(),
        e.auth.clone(),
        Some(e.server.uri()),
        e.server.uri(),
        Some(e.store.clone()),
    ));
    Backend::internal(client, Queries::default(), None, Some(e.store.clone()))
}

#[tokio::test]
async fn rate_limit_short_wait_is_retried() {
    let e = env().await;
    mount_metadata(&e.server).await;
    let content = playlist_content(&[uri("track", 1)], "Mix", 1);
    Mock::given(method("GET"))
        .and(path_regex("^/playlist/v2/playlist/"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "1"))
        .up_to_n_times(1)
        .mount(&e.server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex("^/playlist/v2/playlist/"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(content))
        .mount(&e.server)
        .await;
    let started = Instant::now();
    let (pl, _) = e.backend.catalog.playlist(&id(50)).await.unwrap();
    assert_eq!(pl.name, "Mix");
    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "honoured Retry-After"
    );
}

#[tokio::test]
async fn rate_limit_long_wait_starts_a_cooldown() {
    let e = env().await;
    Mock::given(method("GET"))
        .and(path_regex("^/playlist/v2/playlist/"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "120"))
        .expect(1) // the second call must not reach the server
        .mount(&e.server)
        .await;
    let r = e.backend.catalog.playlist(&id(50)).await;
    assert_eq!(r.unwrap_err(), Error::RateLimited(120));
    let r = e.backend.catalog.playlist(&id(51)).await;
    assert!(matches!(r, Err(Error::RateLimited(s)) if s > 100), "{r:?}");
}

#[tokio::test]
async fn identical_requests_are_coalesced() {
    let e = env().await;
    mount_metadata(&e.server).await;
    Mock::given(method("GET"))
        .and(path_regex("^/playlist/v2/playlist/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(playlist_content(&[uri("track", 1)], "Mix", 1))
                .set_delay(Duration::from_millis(300)),
        )
        .expect(1)
        .mount(&e.server)
        .await;
    let pid = id(50);
    let (a, b, c) = tokio::join!(
        e.backend.catalog.playlist(&pid),
        e.backend.catalog.playlist(&pid),
        e.backend.catalog.playlist(&pid),
    );
    assert!(a.is_ok() && b.is_ok() && c.is_ok());
}

#[tokio::test]
async fn unauthorized_refreshes_the_session_token_once() {
    let e = env().await;
    Mock::given(method("GET"))
        .and(path("/color-lyrics/v2/track/abc"))
        .and(header("authorization", "Bearer session-token-1"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&e.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/color-lyrics/v2/track/abc"))
        .and(header("authorization", "Bearer session-token-2"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("lyrics.json")))
        .mount(&e.server)
        .await;
    let l = e.backend.lyrics.lyrics("abc").await.unwrap().unwrap();
    assert!(l.synced);
    assert_eq!(l.lines[0].text, "Harbor lights are humming low");
    assert_eq!(l.provider.as_deref(), Some("Musixmatch"));
    assert_eq!(e.auth.fetches.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn lyrics_missing_is_none_and_stale_cache_survives_outage() {
    let e = env().await;
    Mock::given(method("GET"))
        .and(path("/color-lyrics/v2/track/none"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&e.server)
        .await;
    assert_eq!(e.backend.lyrics.lyrics("none").await.unwrap(), None);
    assert_eq!(e.backend.lyrics.lyrics("../x").await.unwrap(), None);

    Mock::given(method("GET"))
        .and(path("/user-profile-view/v3/profile/ferro_user"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("profile.json")))
        .mount(&e.server)
        .await;
    let me = e.backend.library.me().await.unwrap();
    assert_eq!(me.display_name, "Ferro Listener");
    assert!(me.is_premium());

    // A stale cache entry is served when Spotify fails.
    use ferrotune::net::client::{Request as ApiRequest, Target};
    Mock::given(method("GET"))
        .and(path("/stale"))
        .respond_with(ResponseTemplate::new(200).set_body_string("cached"))
        .up_to_n_times(1)
        .mount(&e.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/stale"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&e.server)
        .await;
    let req = ApiRequest::get(Target::Spclient("/stale".into()), "text/plain")
        .ttl(Duration::from_secs(1));
    assert_eq!(e.client.send(req.clone()).await.unwrap(), b"cached");
    tokio::time::sleep(Duration::from_millis(2100)).await;
    assert_eq!(
        e.client.send(req).await.unwrap(),
        b"cached",
        "stale-if-error"
    );
}

#[tokio::test]
async fn search_uses_pathfinder_persisted_query() {
    let e = env().await;
    let hash = Queries::default().hash("searchDesktop").unwrap().to_owned();
    Mock::given(method("GET"))
        .and(path("/pathfinder/v1/query"))
        .and(query_param("operationName", "searchDesktop"))
        .and(header("client-token", "client-token-1"))
        .and(header("accept", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("pathfinder_search.json")))
        .expect(1)
        .mount(&e.server)
        .await;
    let r = e.backend.search.search("harbor").await.unwrap();
    assert_eq!(r.tracks.len(), 1, "episodes are dropped");
    assert_eq!(r.tracks[0].name, "Harbor Lights");
    assert_eq!(r.tracks[0].duration_ms, 215_000);
    assert_eq!(r.artists[0].name, "Nova Harbor");
    assert_eq!(r.albums[0].year(), "2021");
    assert_eq!(r.playlists[0].owner, "Ferro");
    let req = &e.server.received_requests().await.unwrap()[0];
    let ext = req
        .url
        .query_pairs()
        .find(|(k, _)| k == "extensions")
        .unwrap()
        .1
        .into_owned();
    assert!(ext.contains(&hash));
}

#[tokio::test]
async fn rejected_persisted_query_falls_back_to_spclient_search() {
    let e = env().await;
    Mock::given(method("GET"))
        .and(path("/pathfinder/v1/query"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(fixture("pathfinder_rejected.json")),
        )
        .mount(&e.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/searchview/km/v4/search/harbor"))
        .and(query_param("country", "SE"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("searchview.json")))
        .expect(1)
        .mount(&e.server)
        .await;
    let r = e.backend.search.search("harbor").await.unwrap();
    assert_eq!(r.tracks[0].name, "Harbor Lights");
}

#[tokio::test]
async fn home_shelves_from_pathfinder() {
    let e = env().await;
    Mock::given(method("GET"))
        .and(path("/pathfinder/v1/query"))
        .and(query_param("operationName", "home"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("pathfinder_home.json")))
        .mount(&e.server)
        .await;
    let h = e.backend.home.home().await.unwrap();
    assert_eq!(h.shelves.len(), 1, "podcast-only shelf dropped");
    assert_eq!(h.shelves[0].title, "Made For Ferro");
    assert!(matches!(&h.shelves[0].items[0], Card::Playlist(p) if p.name == "Daily Mix 1"));
    assert!(matches!(&h.shelves[0].items[1], Card::Album(a) if a.artist_line() == "Nova Harbor"));
}

#[tokio::test]
async fn home_falls_back_to_recently_played() {
    let e = env().await;
    // Album 4aaw... and artist 0Od... from the fixture are mapped to gids.
    Mock::given(method("GET"))
        .and(path("/pathfinder/v1/query"))
        .respond_with(ResponseTemplate::new(400))
        .mount(&e.server)
        .await;
    let rp = fixture("recently_played.json")
        .replace("4aawyAB9vmqN3uQ7FjRGTy", &id(100))
        .replace("0OdUWJ0sBjDrqHygGUXeCF", &id(200));
    Mock::given(method("GET"))
        .and(path("/recently-played/v3/user/ferro_user/recently-played"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rp))
        .mount(&e.server)
        .await;
    mount_metadata(&e.server).await;
    let h = e.backend.home.home().await.unwrap();
    assert_eq!(h.shelves[0].title, "Jump back in");
    let names: Vec<String> = h.shelves[0]
        .items
        .iter()
        .map(|c| match c {
            Card::Album(a) => a.name.clone(),
            Card::Artist(a) => a.name.clone(),
            Card::Playlist(p) => p.name.clone(),
        })
        .collect();
    assert_eq!(names, ["Album 100", "Nova Harbor"]);
}

#[tokio::test]
async fn absurd_retry_after_is_clamped_not_a_panic() {
    let e = env().await;
    Mock::given(method("GET"))
        .and(path_regex("^/playlist/v2/playlist/"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "99999999999999999"))
        .mount(&e.server)
        .await;
    let r = e.backend.catalog.playlist(&id(50)).await;
    let max = ferrotune::net::client::MAX_COOLDOWN.as_secs();
    assert_eq!(r.unwrap_err(), Error::RateLimited(max));
}

#[tokio::test]
async fn cooldown_is_scoped_to_the_endpoint_family() {
    let e = env().await;
    Mock::given(method("GET"))
        .and(path("/pathfinder/v1/query"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "300"))
        .expect(1)
        .mount(&e.server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex("^/searchview/"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "300"))
        .mount(&e.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/collection/v2/write"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&e.server)
        .await;
    // Search is limited (pathfinder and its fallback)...
    assert!(matches!(
        e.backend.search.search("x").await,
        Err(Error::RateLimited(_))
    ));
    assert!(matches!(
        e.backend.search.search("y").await,
        Err(Error::RateLimited(_))
    ));
    // ...but library writes still go through.
    Mock::given(method("POST"))
        .and(path("/collection/v2/paging"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(collection_page(&[], "", "s")))
        .mount(&e.server)
        .await;
    e.backend
        .library
        .set_saved(vec![id(3)], true)
        .await
        .unwrap();
}

#[tokio::test]
async fn dropped_callers_do_not_wedge_the_client() {
    let e = env().await;
    mount_metadata(&e.server).await;
    Mock::given(method("GET"))
        .and(path_regex("^/playlist/v2/playlist/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(playlist_content(&[uri("track", 1)], "Mix", 1))
                .set_delay(Duration::from_millis(400)),
        )
        .mount(&e.server)
        .await;
    // More dropped in-flight callers than concurrency permits.
    for n in 0..8u8 {
        let b = e.backend.clone();
        let pid = id(60 + n);
        let _ = tokio::time::timeout(Duration::from_millis(50), async move {
            b.catalog.playlist(&pid).await
        })
        .await;
    }
    let r =
        tokio::time::timeout(Duration::from_secs(10), e.backend.catalog.playlist(&id(90))).await;
    assert!(matches!(r, Ok(Ok(_))), "client is still usable: {r:?}");
}

#[tokio::test]
async fn graphql_errors_are_not_cached_and_403_falls_back() {
    let e = env().await;
    Mock::given(method("GET"))
        .and(path("/pathfinder/v1/query"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(fixture("pathfinder_rejected.json")),
        )
        .up_to_n_times(1)
        .mount(&e.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/pathfinder/v1/query"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("pathfinder_search.json")))
        .mount(&e.server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex("^/searchview/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("searchview.json")))
        .mount(&e.server)
        .await;
    // First answer is an error body: fallback is used, nothing cached...
    let first = e.backend.search.search("harbor").await.unwrap();
    assert_eq!(first.artists.len(), 0, "searchview fixture has no artists");
    // ...so the second call asks pathfinder again and gets real results.
    let second = e.backend.search.search("harbor").await.unwrap();
    assert_eq!(second.artists.len(), 1);

    // HTTP 403 from pathfinder also falls back.
    let e = env().await;
    Mock::given(method("GET"))
        .and(path("/pathfinder/v1/query"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&e.server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex("^/searchview/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("searchview.json")))
        .expect(1)
        .mount(&e.server)
        .await;
    assert_eq!(
        e.backend.search.search("harbor").await.unwrap().tracks[0].name,
        "Harbor Lights"
    );
}
