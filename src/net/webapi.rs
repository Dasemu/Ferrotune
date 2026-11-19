//! Spotify Web API client (library, search, playlists) over `reqwest`.
//!
//! * Base URL is injectable so integration tests run against `wiremock`.
//! * 401 → one token refresh + retry. 429 → honour `Retry-After` (short
//!   waits are slept, long ones become `Error::RateLimited`). 5xx → one retry.

use std::sync::Arc;
use std::time::Duration;

use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;

use super::model::{
    Album, Artist, ArtistPage, HomeFeed, Library, Page, Playlist, SearchResults, Track, User,
};
use super::paging::Collector;
use super::types as wire;
use crate::auth::oauth::{self, TokenSet, now_secs};
use crate::auth::secrets::Secret;
use crate::error::{Error, Result};

/// Longest `Retry-After` we are willing to sleep inside a request.
const MAX_INLINE_RETRY_AFTER: u64 = 5;
/// Cap for library collections (sidebar).
pub const LIBRARY_CAP: usize = 500;

pub type RefreshCallback = Arc<dyn Fn(&TokenSet) + Send + Sync>;

/// Holds the current token pair and refreshes it on demand.
pub struct TokenManager {
    state: tokio::sync::Mutex<TokenSet>,
    client_id: String,
    accounts_base: String,
    http: reqwest::Client,
    on_refresh: Option<RefreshCallback>,
}

impl TokenManager {
    pub fn new(
        tokens: TokenSet,
        client_id: String,
        accounts_base: String,
        http: reqwest::Client,
        on_refresh: Option<RefreshCallback>,
    ) -> Self {
        Self {
            state: tokio::sync::Mutex::new(tokens),
            client_id,
            accounts_base,
            http,
            on_refresh,
        }
    }

    /// A valid access token, refreshing first if it is about to expire.
    pub async fn access_token(&self) -> Result<Secret> {
        let mut st = self.state.lock().await;
        if st.is_expired(now_secs()) && st.refresh_token.is_some() {
            *st = self.do_refresh(&st).await?;
        }
        Ok(st.access_token.clone())
    }

    /// Refresh after a 401 for a request made with `stale`.
    ///
    /// Single-flight: if another request already refreshed (the stored token
    /// differs from `stale`), the new token is returned without hitting the
    /// token endpoint again.
    pub async fn refresh_now(&self, stale: &Secret) -> Result<Secret> {
        let mut st = self.state.lock().await;
        if st.access_token != *stale {
            return Ok(st.access_token.clone());
        }
        if st.refresh_token.is_none() {
            return Err(Error::revoked("Session expired. Please log in again."));
        }
        *st = self.do_refresh(&st).await?;
        Ok(st.access_token.clone())
    }

    pub async fn snapshot(&self) -> TokenSet {
        self.state.lock().await.clone()
    }

    async fn do_refresh(&self, current: &TokenSet) -> Result<TokenSet> {
        let Some(rt) = current.refresh_token.as_ref() else {
            return Err(Error::revoked("Session expired. Please log in again."));
        };
        tracing::info!("refreshing Web API access token");
        let new = oauth::refresh(&self.http, &self.accounts_base, &self.client_id, rt).await?;
        if let Some(cb) = &self.on_refresh {
            cb(&new);
        }
        Ok(new)
    }
}

#[derive(Clone)]
pub struct WebApi {
    http: reqwest::Client,
    base: String,
    tokens: Arc<TokenManager>,
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("Ferrotune/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default()
}

fn parse_retry_after(v: Option<&reqwest::header::HeaderValue>) -> u64 {
    v.and_then(|h| h.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(1)
}

impl WebApi {
    pub fn new(http: reqwest::Client, base: impl Into<String>, tokens: Arc<TokenManager>) -> Self {
        Self {
            http,
            base: base.into().trim_end_matches('/').to_owned(),
            tokens,
        }
    }

    pub fn tokens(&self) -> &Arc<TokenManager> {
        &self.tokens
    }

    /// Send a request, returning the body bytes (empty for 204).
    async fn request(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Vec<u8>> {
        let url = format!("{}{}", self.base, path);
        let mut token = self.tokens.access_token().await?;
        let mut refreshed = false;
        let mut attempts = 0u32;
        loop {
            attempts += 1;
            let mut req = self
                .http
                .request(method.clone(), &url)
                .bearer_auth(token.expose())
                .query(query);
            if method != Method::GET {
                // Spotify answers 411 to body-less PUT/DELETE without it.
                req = req.header(reqwest::header::CONTENT_LENGTH, "0");
            }
            let resp = req.send().await?;
            let status = resp.status();
            tracing::debug!(%method, path, status = status.as_u16(), "web api");
            if status.is_success() {
                return Ok(resp.bytes().await?.to_vec());
            }
            match status {
                StatusCode::UNAUTHORIZED if !refreshed => {
                    refreshed = true;
                    token = self.tokens.refresh_now(&token).await?;
                }
                StatusCode::TOO_MANY_REQUESTS => {
                    let wait = parse_retry_after(resp.headers().get(reqwest::header::RETRY_AFTER));
                    if wait > MAX_INLINE_RETRY_AFTER || attempts > 3 {
                        return Err(Error::RateLimited(wait));
                    }
                    tokio::time::sleep(Duration::from_secs(wait)).await;
                }
                s if s.is_server_error() && attempts < 2 => {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
                s => {
                    let body = resp.bytes().await.unwrap_or_default();
                    let message = serde_json::from_slice::<wire::ApiErrorBody>(&body)
                        .map(|b| b.error.message)
                        .unwrap_or_else(|_| s.canonical_reason().unwrap_or("error").to_owned());
                    if s == StatusCode::UNAUTHORIZED {
                        return Err(Error::revoked("Session expired. Please log in again."));
                    }
                    return Err(Error::Http {
                        status: s.as_u16(),
                        message,
                    });
                }
            }
        }
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T> {
        let body = self.request(Method::GET, path, query).await?;
        serde_json::from_slice(&body).map_err(|e| Error::Protocol(format!("{path}: {e}")))
    }

    pub async fn me(&self) -> Result<User> {
        Ok(self.get::<wire::UserProfile>("/me", &[]).await?.into())
    }

    async fn collect_offset<W, T>(
        &self,
        path: &str,
        limit: u32,
        cap: usize,
        extra: &[(&str, String)],
        convert: impl Fn(W) -> Option<T> + Copy,
    ) -> Result<Vec<T>>
    where
        W: DeserializeOwned,
    {
        let mut collector = Collector::new(cap);
        let mut offset = 0;
        loop {
            let mut q = vec![("limit", limit.to_string()), ("offset", offset.to_string())];
            q.extend(extra.iter().cloned());
            let page: wire::Paging<W> = self.get(path, &q).await?;
            let page = page.into_page(convert);
            let next = page.next_offset();
            if !collector.push(page.items, next.is_some()) {
                break;
            }
            match next {
                Some(n) => offset = n,
                None => break,
            }
        }
        Ok(collector.items)
    }

    pub async fn my_playlists(&self) -> Result<Vec<Playlist>> {
        self.collect_offset(
            "/me/playlists",
            50,
            LIBRARY_CAP,
            &[],
            |p: wire::Playlist| Some(p.into()),
        )
        .await
    }

    pub async fn saved_albums(&self) -> Result<Vec<Album>> {
        self.collect_offset("/me/albums", 50, LIBRARY_CAP, &[], |s: wire::SavedAlbum| {
            Some(s.album.into())
        })
        .await
    }

    /// Followed artists use cursor paging (`after`).
    pub async fn followed_artists(&self) -> Result<Vec<Artist>> {
        let mut collector = Collector::new(LIBRARY_CAP);
        let mut after: Option<String> = None;
        loop {
            let mut q = vec![("type", "artist".to_owned()), ("limit", "50".to_owned())];
            if let Some(a) = &after {
                q.push(("after", a.clone()));
            }
            let resp: wire::FollowedArtists = self.get("/me/following", &q).await?;
            let next_cursor = resp
                .artists
                .cursors
                .and_then(|c| c.after)
                .filter(|_| resp.artists.next.is_some());
            let items: Vec<Artist> = resp
                .artists
                .items
                .into_iter()
                .flatten()
                .map(Into::into)
                .collect();
            if !collector.push(items, next_cursor.is_some()) {
                break;
            }
            after = next_cursor;
        }
        Ok(collector.items)
    }

    /// One page of the user's liked songs.
    pub async fn saved_tracks(&self, offset: u32) -> Result<Page<Track>> {
        let p: wire::Paging<wire::SavedTrack> = self
            .get(
                "/me/tracks",
                &[("limit", "50".into()), ("offset", offset.to_string())],
            )
            .await?;
        Ok(p.into_page(wire::SavedTrack::into_model))
    }

    pub async fn library(&self) -> Result<Library> {
        let q1 = [("limit", "1".to_owned())];
        let (playlists, albums, artists, liked) = tokio::join!(
            self.my_playlists(),
            self.saved_albums(),
            self.followed_artists(),
            self.get::<wire::Paging<wire::SavedTrack>>("/me/tracks", &q1),
        );
        Ok(Library {
            playlists: playlists?,
            albums: albums?,
            artists: artists?,
            liked_total: liked.map(|p| p.total).unwrap_or(0),
        })
    }

    pub async fn playlist(&self, id: &str) -> Result<(Playlist, Page<Track>)> {
        let p: wire::Playlist = self.get(&format!("/playlists/{id}"), &[]).await?;
        let (pl, page) = p.into_model();
        let page = match page {
            Some(p) => p,
            None => self.playlist_tracks(id, 0).await?,
        };
        Ok((pl, page))
    }

    pub async fn playlist_tracks(&self, id: &str, offset: u32) -> Result<Page<Track>> {
        let p: wire::Paging<wire::SavedTrack> = self
            .get(
                &format!("/playlists/{id}/tracks"),
                &[("limit", "100".into()), ("offset", offset.to_string())],
            )
            .await?;
        Ok(p.into_page(wire::SavedTrack::into_model))
    }

    pub async fn album(&self, id: &str) -> Result<Album> {
        let a: wire::Album = self.get(&format!("/albums/{id}"), &[]).await?;
        let (mut album, page) = a.into_model();
        let album_ref = album.as_ref();
        if let Some(mut p) = page {
            album.tracks.append(&mut p.items);
            let mut next = super::paging::next_offset(p.offset, album.tracks.len(), p.total);
            while let Some(off) = next {
                let more: wire::Paging<wire::Track> = self
                    .get(
                        &format!("/albums/{id}/tracks"),
                        &[("limit", "50".into()), ("offset", off.to_string())],
                    )
                    .await?;
                let more = more.into_page(|t| {
                    let mut t = t.into_model()?;
                    t.album.get_or_insert_with(|| album_ref.clone());
                    Some(t)
                });
                next = more.next_offset();
                album.tracks.extend(more.items);
            }
        }
        Ok(album)
    }

    pub async fn artist_page(&self, id: &str) -> Result<ArtistPage> {
        let (p_artist, p_top, p_albums) = (
            format!("/artists/{id}"),
            format!("/artists/{id}/top-tracks"),
            format!("/artists/{id}/albums"),
        );
        let q_top = [("market", "from_token".to_owned())];
        let q_albums = [
            ("include_groups", "album,single".to_owned()),
            ("limit", "20".to_owned()),
        ];
        let (artist, top, albums) = tokio::join!(
            self.get::<wire::FullArtist>(&p_artist, &[]),
            self.get::<wire::TopTracks>(&p_top, &q_top),
            self.get::<wire::Paging<wire::Album>>(&p_albums, &q_albums),
        );
        Ok(ArtistPage {
            artist: artist?.into(),
            // Top tracks may be unavailable for some apps; degrade gracefully.
            top_tracks: top
                .map(|t| {
                    t.tracks
                        .into_iter()
                        .filter_map(wire::Track::into_model)
                        .collect()
                })
                .unwrap_or_default(),
            albums: albums
                .map(|p| p.into_page(|a| Some(a.into())).items)
                .unwrap_or_default(),
        })
    }

    pub async fn search(&self, query: &str, limit: u32) -> Result<SearchResults> {
        let r: wire::SearchResponse = self
            .get(
                "/search",
                &[
                    ("q", query.to_owned()),
                    ("type", "track,artist,album,playlist".into()),
                    ("limit", limit.to_string()),
                ],
            )
            .await?;
        Ok(r.into_model(query))
    }

    pub async fn home(&self) -> Result<HomeFeed> {
        let q30 = [("limit", "30".to_owned())];
        let q10 = [("limit", "10".to_owned())];
        let (recent, top_artists, top_tracks) = tokio::join!(
            self.get::<wire::CursorPaging<wire::PlayHistory>>("/me/player/recently-played", &q30),
            self.get::<wire::Paging<wire::FullArtist>>("/me/top/artists", &q10),
            self.get::<wire::Paging<wire::Track>>("/me/top/tracks", &q10),
        );
        Ok(HomeFeed {
            recent_tracks: recent
                .map(|p| {
                    p.items
                        .into_iter()
                        .flatten()
                        .filter_map(|h| h.track.into_model())
                        .collect()
                })
                .unwrap_or_default(),
            top_artists: top_artists?.into_page(|a| Some(a.into())).items,
            top_tracks: top_tracks
                .map(|p| p.into_page(wire::Track::into_model).items)
                .unwrap_or_default(),
        })
    }

    pub async fn contains_saved_tracks(&self, ids: &[String]) -> Result<Vec<bool>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        self.get("/me/tracks/contains", &[("ids", ids.join(","))])
            .await
    }

    pub async fn set_saved(&self, ids: &[String], saved: bool) -> Result<()> {
        let method = if saved { Method::PUT } else { Method::DELETE };
        self.request(method, "/me/tracks", &[("ids", ids.join(","))])
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_parsing() {
        let h = reqwest::header::HeaderValue::from_static("7");
        assert_eq!(parse_retry_after(Some(&h)), 7);
        let h = reqwest::header::HeaderValue::from_static("soon");
        assert_eq!(parse_retry_after(Some(&h)), 1);
        assert_eq!(parse_retry_after(None), 1);
    }
}
