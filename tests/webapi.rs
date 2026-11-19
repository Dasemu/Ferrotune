//! Integration tests: the Web API client and the OAuth token endpoint run
//! against a local `wiremock` server replaying hand-written fixtures shaped
//! like the documented Spotify responses. No real account is involved.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ferrotune::auth::oauth::{self, TokenSet, now_secs};
use ferrotune::auth::secrets::Secret;
use ferrotune::error::Error;
use ferrotune::net::webapi::{TokenManager, WebApi};
use wiremock::matchers::{body_string_contains, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture(name: &str) -> serde_json::Value {
    let p = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn tokens(access: &str, expires_in: i64) -> TokenSet {
    TokenSet {
        access_token: Secret::new(access),
        refresh_token: Some(Secret::new("refresh-1")),
        expires_at: (now_secs() as i64 + expires_in) as u64,
        scope: "streaming".into(),
    }
}

fn client(server: &MockServer, t: TokenSet, refreshed: Option<Arc<AtomicUsize>>) -> WebApi {
    let http = reqwest::Client::new();
    let cb = refreshed.map(|c| {
        Arc::new(move |_: &TokenSet| {
            c.fetch_add(1, Ordering::SeqCst);
        }) as ferrotune::net::webapi::RefreshCallback
    });
    let mgr = TokenManager::new(t, "client-id".into(), server.uri(), http.clone(), cb);
    WebApi::new(http, server.uri(), Arc::new(mgr))
}

async fn json(server: &MockServer, p: &str, name: &str) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture(name)))
        .mount(server)
        .await;
}

#[tokio::test]
async fn profile_and_bearer_header() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/me"))
        .and(header("authorization", "Bearer tok"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("me.json")))
        .expect(1)
        .mount(&server)
        .await;
    let me = client(&server, tokens("tok", 3600), None)
        .me()
        .await
        .unwrap();
    assert_eq!(me.display_name, "Test User");
    assert!(me.is_premium());
    assert_eq!(me.image.as_deref(), Some("https://i.scdn.co/image/me"));
}

#[tokio::test]
async fn library_follows_offset_and_cursor_paging() {
    let server = MockServer::start().await;
    for (offset, file) in [("0", "me_playlists_0.json"), ("50", "me_playlists_50.json")] {
        Mock::given(method("GET"))
            .and(path("/me/playlists"))
            .and(query_param("offset", offset))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture(file)))
            .expect(1)
            .mount(&server)
            .await;
    }
    json(&server, "/me/albums", "me_albums.json").await;
    Mock::given(method("GET"))
        .and(path("/me/following"))
        .and(query_param("after", "artist1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("following_2.json")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/me/following"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("following_1.json")))
        .mount(&server)
        .await;
    json(&server, "/me/tracks", "saved_tracks.json").await;

    let lib = client(&server, tokens("tok", 3600), None)
        .library()
        .await
        .unwrap();
    assert_eq!(lib.playlists.len(), 60);
    assert_eq!(lib.playlists[0].description, "Mix & match", "HTML stripped");
    assert_eq!(lib.playlists[0].owner, "Owner Name");
    assert_eq!(lib.playlists[3].total_tracks, 13);
    assert_eq!(lib.albums.len(), 3);
    assert_eq!(lib.albums[1].kind, ferrotune::net::model::AlbumKind::Single);
    assert_eq!(lib.artists.len(), 3);
    assert_eq!(lib.artists[1].followers, Some(1000));
    assert_eq!(lib.liked_total, 120);
}

#[tokio::test]
async fn playlist_skips_null_and_episode_items_and_pages() {
    let server = MockServer::start().await;
    json(&server, "/playlists/pl1", "playlist.json").await;
    json(&server, "/playlists/pl1/tracks", "playlist_tracks_100.json").await;
    let api = client(&server, tokens("tok", 3600), None);
    let (pl, page) = api.playlist("pl1").await.unwrap();
    assert_eq!(pl.name, "Playlist 1");
    assert_eq!(pl.total_tracks, 104);
    // track(1) + local file; null and episode entries dropped.
    assert_eq!(page.items.len(), 2);
    assert_eq!(
        page.items[0].added_at.as_deref(),
        Some("2025-01-01T00:00:00Z")
    );
    assert!(!page.items[1].playable, "local files are not playable");
    // Paging continues on raw entry count (4), not on filtered items.
    assert_eq!(page.next_offset(), Some(4));
    let more = api.playlist_tracks("pl1", 100).await.unwrap();
    assert_eq!(more.items[0].id, "track7", "`item` alias accepted");
    assert_eq!(more.next_offset(), Some(101), "fixture claims 104 total");
}

#[tokio::test]
async fn album_fetches_remaining_tracks() {
    let server = MockServer::start().await;
    json(&server, "/albums/album0", "album.json").await;
    Mock::given(method("GET"))
        .and(path("/albums/album0/tracks"))
        .and(query_param("offset", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("album_tracks_2.json")))
        .expect(1)
        .mount(&server)
        .await;
    let a = client(&server, tokens("tok", 3600), None)
        .album("album0")
        .await
        .unwrap();
    assert_eq!(a.tracks.len(), 3);
    assert_eq!(a.year(), "2023");
    assert!(
        a.tracks
            .iter()
            .all(|t| t.album.as_ref().is_some_and(|al| al.id == "album0")),
        "simplified tracks get the album attached"
    );
}

#[tokio::test]
async fn artist_page_and_search_and_home() {
    let server = MockServer::start().await;
    json(&server, "/artists/artist1", "artist.json").await;
    json(
        &server,
        "/artists/artist1/top-tracks",
        "artist_top_tracks.json",
    )
    .await;
    json(&server, "/artists/artist1/albums", "artist_albums.json").await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("q", "hello world"))
        .and(query_param("type", "track,artist,album,playlist"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("search.json")))
        .mount(&server)
        .await;
    json(
        &server,
        "/me/player/recently-played",
        "recently_played.json",
    )
    .await;
    json(&server, "/me/top/artists", "top_artists.json").await;
    json(&server, "/me/top/tracks", "top_tracks.json").await;
    let api = client(&server, tokens("tok", 3600), None);

    let p = api.artist_page("artist1").await.unwrap();
    assert_eq!(p.artist.followers, Some(123_456));
    assert_eq!(p.top_tracks.len(), 3);
    assert_eq!(p.albums.len(), 2);

    let r = api.search("hello world", 10).await.unwrap();
    assert_eq!(r.tracks.len(), 2);
    assert_eq!(r.playlists.len(), 2, "null search entries are skipped");
    assert_eq!(r.query, "hello world");

    let h = api.home().await.unwrap();
    assert_eq!(h.recent_tracks.len(), 3);
    assert_eq!(h.top_artists.len(), 2);
    assert_eq!(h.top_tracks.len(), 2);
}

#[tokio::test]
async fn unauthorized_refreshes_once_and_retries() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/me"))
        .and(header("authorization", "Bearer stale"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": {"status": 401, "message": "The access token expired"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/me"))
        .and(header("authorization", "Bearer BQD-fresh-access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("me.json")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=refresh-1"))
        .and(body_string_contains("client_id=client-id"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("token.json")))
        .expect(1)
        .mount(&server)
        .await;
    let count = Arc::new(AtomicUsize::new(0));
    let api = client(&server, tokens("stale", 3600), Some(count.clone()));
    api.me().await.unwrap();
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "refresh persisted via callback"
    );
    let snap = api.tokens().snapshot().await;
    assert_eq!(snap.refresh_token.unwrap().expose(), "AQD-rotated-refresh");
}

#[tokio::test]
async fn expired_token_is_refreshed_before_the_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("token.json")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/me"))
        .and(header("authorization", "Bearer BQD-fresh-access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("me.json")))
        .expect(1)
        .mount(&server)
        .await;
    client(&server, tokens("old", -10), None)
        .me()
        .await
        .unwrap();
}

#[tokio::test]
async fn invalid_grant_means_revoked() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/me"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": "invalid_grant", "error_description": "Refresh token revoked"
        })))
        .mount(&server)
        .await;
    let err = client(&server, tokens("x", 3600), None)
        .me()
        .await
        .unwrap_err();
    assert!(err.is_revoked(), "{err:?}");
}

#[tokio::test]
async fn rate_limit_short_wait_is_retried_long_wait_is_reported() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/me"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "1"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    json(&server, "/me", "me.json").await;
    let api = client(&server, tokens("tok", 3600), None);
    assert!(api.me().await.is_ok());

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/me"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "30"))
        .mount(&server)
        .await;
    let err = client(&server, tokens("tok", 3600), None)
        .me()
        .await
        .unwrap_err();
    assert_eq!(err, Error::RateLimited(30));
}

#[tokio::test]
async fn server_error_retried_once_and_404_reported() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/albums/x"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    json(&server, "/albums/x", "album.json").await;
    json(&server, "/albums/x/tracks", "album_tracks_2.json").await;
    let api = client(&server, tokens("tok", 3600), None);
    api.album("x").await.unwrap();

    Mock::given(method("GET"))
        .and(path("/albums/missing"))
        .respond_with(ResponseTemplate::new(404).set_body_json(fixture("error_404.json")))
        .mount(&server)
        .await;
    let err = api.album("missing").await.unwrap_err();
    assert_eq!(
        err,
        Error::Http {
            status: 404,
            message: "Resource not found".into()
        }
    );
}

#[tokio::test]
async fn save_and_unsave_tracks() {
    let server = MockServer::start().await;
    for m in ["PUT", "DELETE"] {
        Mock::given(method(m))
            .and(path("/me/tracks"))
            .and(query_param("ids", "a,b"))
            .and(header("content-length", "0"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/me/tracks/contains"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([true, false])))
        .mount(&server)
        .await;
    let api = client(&server, tokens("tok", 3600), None);
    let ids = vec!["a".to_string(), "b".to_string()];
    api.set_saved(&ids, true).await.unwrap();
    api.set_saved(&ids, false).await.unwrap();
    assert_eq!(
        api.contains_saved_tracks(&ids).await.unwrap(),
        vec![true, false]
    );
    assert!(api.contains_saved_tracks(&[]).await.unwrap().is_empty());
}

#[tokio::test]
async fn pkce_code_exchange_sends_verifier() {
    let server = MockServer::start().await;
    let pkce = oauth::Pkce::from_verifier("v".repeat(50));
    Mock::given(method("POST"))
        .and(path("/api/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code=the-code"))
        .and(body_string_contains(format!(
            "code_verifier={}",
            "v".repeat(50)
        )))
        .and(body_string_contains(
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A8898%2Flogin",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("token.json")))
        .expect(1)
        .mount(&server)
        .await;
    let t = oauth::exchange_code(
        &reqwest::Client::new(),
        &server.uri(),
        "client-id",
        &Secret::new("the-code"),
        "http://127.0.0.1:8898/login",
        &pkce.verifier,
    )
    .await
    .unwrap();
    assert_eq!(t.access_token.expose(), "BQD-fresh-access");
    assert!(!t.is_expired(now_secs()));
}

#[tokio::test]
async fn image_fetch_and_decode() {
    let server = MockServer::start().await;
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        640,
        640,
        image::Rgba([0, 0, 255, 255]),
    ))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .unwrap();
    Mock::given(method("GET"))
        .and(path("/image/abc"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(png))
        .mount(&server)
        .await;
    let img = ferrotune::net::images::fetch(
        &reqwest::Client::new(),
        &format!("{}/image/abc", server.uri()),
        300,
    )
    .await
    .unwrap();
    assert_eq!((img.width, img.height), (300, 300));
    assert_eq!(img.average, [0, 0, 255]);
}

#[tokio::test]
async fn oversized_image_is_rejected() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/image/huge"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; 9 * 1024 * 1024]))
        .mount(&server)
        .await;
    let err = ferrotune::net::images::fetch(
        &reqwest::Client::new(),
        &format!("{}/image/huge", server.uri()),
        64,
    )
    .await
    .unwrap_err();
    assert_eq!(err, Error::Protocol("image too large".into()));
}

#[tokio::test]
async fn concurrent_401s_refresh_only_once() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/me"))
        .and(header("authorization", "Bearer stale"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/me"))
        .and(header("authorization", "Bearer BQD-fresh-access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("me.json")))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("token.json")))
        .expect(1)
        .mount(&server)
        .await;
    let api = client(&server, tokens("stale", 3600), None);
    let (a, b, c) = tokio::join!(api.me(), api.me(), api.me());
    assert!(a.is_ok() && b.is_ok() && c.is_ok());
}
