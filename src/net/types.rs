//! Spotify Web API wire types (serde) and their conversion to domain types.
//!
//! The wire structs are deliberately lenient: every field that Spotify may
//! omit or null is an `Option` or has a default, and list entries that may
//! be `null` (search results, removed playlist items) are `Option<T>`.

use serde::Deserialize;

use super::model;

#[derive(Debug, Clone, Deserialize)]
pub struct Image {
    pub url: String,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

impl From<Image> for model::Image {
    fn from(i: Image) -> Self {
        Self {
            url: i.url,
            width: i.width,
            height: i.height,
        }
    }
}

fn images(v: Option<Vec<Image>>) -> Vec<model::Image> {
    v.unwrap_or_default().into_iter().map(Into::into).collect()
}

/// Parse list entries one by one so a single malformed or unknown entry
/// (Spotify adds item types over time) drops that entry, not the page.
fn lenient_items<'de, D, T>(d: D) -> Result<Vec<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let raw: Option<Vec<serde_json::Value>> = Option::deserialize(d)?;
    Ok(raw
        .unwrap_or_default()
        .into_iter()
        .map(|v| serde_json::from_value(v).ok())
        .collect())
}

/// `null` → `T::default()` (local files have `"id": null`).
fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Clone, Deserialize)]
pub struct Paging<T> {
    #[serde(default = "Vec::new", deserialize_with = "lenient_items")]
    #[serde(bound(deserialize = "T: serde::de::DeserializeOwned"))]
    pub items: Vec<Option<T>>,
    #[serde(default)]
    pub offset: u32,
    #[serde(default)]
    pub total: u32,
    #[serde(default)]
    pub next: Option<String>,
}

impl<T> Paging<T> {
    pub fn into_page<U>(self, f: impl FnMut(T) -> Option<U>) -> model::Page<U> {
        let raw_len = self.items.len();
        let mut items: Vec<U> = self.items.into_iter().flatten().filter_map(f).collect();
        items.shrink_to_fit();
        // `total`/`offset` describe raw entries; keep paging on raw length.
        model::Page {
            items,
            offset: self.offset,
            total: self.total.max(
                self.offset
                    .saturating_add(u32::try_from(raw_len).unwrap_or(u32::MAX)),
            ),
            raw_len: u32::try_from(raw_len).unwrap_or(u32::MAX),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Cursors {
    #[serde(default)]
    pub after: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CursorPaging<T> {
    #[serde(default = "Vec::new", deserialize_with = "lenient_items")]
    #[serde(bound(deserialize = "T: serde::de::DeserializeOwned"))]
    pub items: Vec<Option<T>>,
    #[serde(default)]
    pub next: Option<String>,
    #[serde(default)]
    pub cursors: Option<Cursors>,
    #[serde(default)]
    pub total: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SimpleArtist {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
}

impl From<SimpleArtist> for model::ArtistRef {
    fn from(a: SimpleArtist) -> Self {
        Self {
            id: a.id.unwrap_or_default(),
            name: a.name,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Followers {
    #[serde(default)]
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FullArtist {
    pub id: String,
    #[serde(default)]
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub images: Option<Vec<Image>>,
    #[serde(default)]
    pub followers: Option<Followers>,
    #[serde(default)]
    pub genres: Vec<String>,
}

impl From<FullArtist> for model::Artist {
    fn from(a: FullArtist) -> Self {
        Self {
            uri: if a.uri.is_empty() {
                format!("spotify:artist:{}", a.id)
            } else {
                a.uri
            },
            id: a.id,
            name: a.name,
            images: images(a.images),
            followers: a.followers.and_then(|f| f.total),
            genres: a.genres,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Album {
    #[serde(default, deserialize_with = "null_default")]
    pub id: String,
    #[serde(default)]
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub album_type: Option<String>,
    #[serde(default)]
    pub artists: Vec<SimpleArtist>,
    #[serde(default)]
    pub images: Option<Vec<Image>>,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub total_tracks: Option<u32>,
    /// Present on the full album object only.
    #[serde(default)]
    pub tracks: Option<Paging<Track>>,
}

fn album_kind(s: Option<&str>) -> model::AlbumKind {
    match s {
        Some("single") => model::AlbumKind::Single,
        Some("compilation") => model::AlbumKind::Compilation,
        _ => model::AlbumKind::Album,
    }
}

impl Album {
    /// Convert, returning the album and the first page of its tracks.
    pub fn into_model(self) -> (model::Album, Option<model::Page<model::Track>>) {
        let album_ref = model::AlbumRef {
            id: self.id.clone(),
            name: self.name.clone(),
            images: images(self.images.clone()),
        };
        let page = self.tracks.map(|p| {
            p.into_page(|t| {
                let mut t = t.into_model()?;
                // Album track objects are "simplified" and lack the album.
                if t.album.is_none() {
                    t.album = Some(album_ref.clone());
                }
                Some(t)
            })
        });
        let album = model::Album {
            uri: if self.uri.is_empty() {
                format!("spotify:album:{}", self.id)
            } else {
                self.uri
            },
            kind: album_kind(self.album_type.as_deref()),
            id: self.id,
            name: self.name,
            artists: self.artists.into_iter().map(Into::into).collect(),
            images: album_ref.images,
            release_date: self.release_date.unwrap_or_default(),
            total_tracks: self.total_tracks.unwrap_or(0),
            tracks: Vec::new(),
        };
        (album, page)
    }
}

impl From<Album> for model::Album {
    fn from(a: Album) -> Self {
        let (mut album, page) = a.into_model();
        if let Some(p) = page {
            album.tracks = p.items;
        }
        album
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Track {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub uri: Option<String>,
    pub name: String,
    /// "track" or "episode" (playlists can contain podcast episodes).
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub artists: Vec<SimpleArtist>,
    #[serde(default)]
    pub album: Option<Album>,
    #[serde(default)]
    pub duration_ms: u32,
    #[serde(default)]
    pub explicit: bool,
    #[serde(default)]
    pub is_playable: Option<bool>,
    #[serde(default)]
    pub is_local: bool,
}

impl Track {
    /// `None` for episodes and other non-track entries.
    pub fn into_model(self) -> Option<model::Track> {
        if self.kind.as_deref().is_some_and(|k| k != "track") {
            return None;
        }
        let id = self.id.unwrap_or_default();
        let uri = self.uri.unwrap_or_else(|| format!("spotify:track:{id}"));
        let playable = !self.is_local && !id.is_empty() && self.is_playable.unwrap_or(true);
        Some(model::Track {
            id,
            uri,
            name: self.name,
            artists: self.artists.into_iter().map(Into::into).collect(),
            album: self.album.map(|a| model::AlbumRef {
                id: a.id,
                name: a.name,
                images: images(a.images),
            }),
            duration_ms: self.duration_ms,
            explicit: self.explicit,
            playable,
            added_at: None,
        })
    }
}

/// Entry of a playlist's track list or of the user's saved tracks.
/// Newer API revisions call the field `item`; both are accepted.
#[derive(Debug, Clone, Deserialize)]
pub struct SavedTrack {
    #[serde(default)]
    pub added_at: Option<String>,
    #[serde(default, alias = "item")]
    pub track: Option<Track>,
}

impl SavedTrack {
    pub fn into_model(self) -> Option<model::Track> {
        let mut t = self.track?.into_model()?;
        t.added_at = self.added_at;
        Some(t)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SavedAlbum {
    pub album: Album,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlayHistory {
    pub track: Track,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Owner {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TracksRef {
    #[serde(default)]
    pub total: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Playlist {
    pub id: String,
    #[serde(default)]
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub owner: Option<Owner>,
    #[serde(default)]
    pub images: Option<Vec<Image>>,
    /// Simplified playlists only carry `{href,total}`; full ones a page.
    /// Newer API revisions call the field `items`.
    #[serde(default, alias = "items")]
    pub tracks: Option<serde_json::Value>,
}

impl Playlist {
    pub fn into_model(self) -> (model::Playlist, Option<model::Page<model::Track>>) {
        let mut total = 0;
        let mut page = None;
        if let Some(v) = self.tracks {
            if v.get("items").is_some() {
                if let Ok(p) = serde_json::from_value::<Paging<SavedTrack>>(v) {
                    total = p.total;
                    page = Some(p.into_page(SavedTrack::into_model));
                }
            } else if let Ok(t) = serde_json::from_value::<TracksRef>(v) {
                total = t.total;
            }
        }
        let owner = self
            .owner
            .map(|o| o.display_name.or(o.id).unwrap_or_default())
            .unwrap_or_default();
        let pl = model::Playlist {
            uri: if self.uri.is_empty() {
                format!("spotify:playlist:{}", self.id)
            } else {
                self.uri
            },
            id: self.id,
            name: self.name,
            description: strip_html(&self.description.unwrap_or_default()),
            owner,
            images: images(self.images),
            total_tracks: total,
        };
        (pl, page)
    }
}

impl From<Playlist> for model::Playlist {
    fn from(p: Playlist) -> Self {
        p.into_model().0
    }
}

/// Playlist descriptions may contain `<a href>` tags and HTML entities.
pub fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserProfile {
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub images: Option<Vec<Image>>,
    #[serde(default)]
    pub product: Option<String>,
}

impl From<UserProfile> for model::User {
    fn from(u: UserProfile) -> Self {
        let image = model::pick_image(&images(u.images), 64).map(|i| i.url.clone());
        Self {
            display_name: u.display_name.unwrap_or_else(|| u.id.clone()),
            id: u.id,
            image,
            product: u.product,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchResponse {
    #[serde(default)]
    pub tracks: Option<Paging<Track>>,
    #[serde(default)]
    pub artists: Option<Paging<FullArtist>>,
    #[serde(default)]
    pub albums: Option<Paging<Album>>,
    #[serde(default)]
    pub playlists: Option<Paging<Playlist>>,
}

impl SearchResponse {
    pub fn into_model(self, query: &str) -> model::SearchResults {
        model::SearchResults {
            query: query.to_owned(),
            tracks: self
                .tracks
                .map(|p| p.into_page(Track::into_model).items)
                .unwrap_or_default(),
            artists: self
                .artists
                .map(|p| p.into_page(|a| Some(a.into())).items)
                .unwrap_or_default(),
            albums: self
                .albums
                .map(|p| p.into_page(|a| Some(a.into())).items)
                .unwrap_or_default(),
            playlists: self
                .playlists
                .map(|p| p.into_page(|p| Some(p.into())).items)
                .unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FollowedArtists {
    pub artists: CursorPaging<FullArtist>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TopTracks {
    pub tracks: Vec<Track>,
}

/// `{"error": {"status": 401, "message": "..."}}`
#[derive(Debug, Clone, Deserialize)]
pub struct ApiErrorBody {
    pub error: ApiError,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiError {
    #[serde(default)]
    pub status: u16,
    #[serde(default)]
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_html_handles_links_and_entities() {
        assert_eq!(
            strip_html(r#"Best of <a href="spotify:x">Rock</a> &amp; Roll"#),
            "Best of Rock & Roll"
        );
    }

    #[test]
    fn episodes_and_local_files() {
        let ep: Track = serde_json::from_str(r#"{"name":"Ep","type":"episode"}"#).unwrap();
        assert!(ep.into_model().is_none());
        let local: Track = serde_json::from_str(
            r#"{"name":"Local","type":"track","is_local":true,"uri":"spotify:local:x"}"#,
        )
        .unwrap();
        let t = local.into_model().unwrap();
        assert!(!t.playable);
    }
}
