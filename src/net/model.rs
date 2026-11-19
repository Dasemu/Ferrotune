//! Domain types used by the app and the views. Produced from wire types
//! (`net::types`), from the SQLite cache, or from `mock.rs`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Pick the smallest image that is at least `min_px` wide, else the largest.
pub fn pick_image(images: &[Image], min_px: u32) -> Option<&Image> {
    let mut sorted: Vec<&Image> = images.iter().collect();
    sorted.sort_by_key(|i| i.width.unwrap_or(0));
    sorted
        .iter()
        .find(|i| i.width.unwrap_or(0) >= min_px)
        .copied()
        .or_else(|| sorted.last().copied())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistRef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumRef {
    pub id: String,
    pub name: String,
    pub images: Vec<Image>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub uri: String,
    pub name: String,
    pub artists: Vec<ArtistRef>,
    pub album: Option<AlbumRef>,
    pub duration_ms: u32,
    pub explicit: bool,
    pub playable: bool,
    /// ISO-8601 date the track was added (playlists / liked songs).
    pub added_at: Option<String>,
}

impl Track {
    pub fn artist_line(&self) -> String {
        self.artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn images(&self) -> &[Image] {
        self.album.as_ref().map_or(&[], |a| a.images.as_slice())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AlbumKind {
    #[default]
    Album,
    Single,
    Compilation,
}

impl AlbumKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Album => "Album",
            Self::Single => "Single",
            Self::Compilation => "Compilation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Album {
    pub id: String,
    pub uri: String,
    pub name: String,
    pub artists: Vec<ArtistRef>,
    pub images: Vec<Image>,
    pub release_date: String,
    pub kind: AlbumKind,
    pub total_tracks: u32,
    /// Empty for simplified albums (lists); filled for the album view.
    pub tracks: Vec<Track>,
}

impl Album {
    pub fn year(&self) -> &str {
        self.release_date.get(..4).unwrap_or(&self.release_date)
    }

    pub fn artist_line(&self) -> String {
        self.artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn as_ref(&self) -> AlbumRef {
        AlbumRef {
            id: self.id.clone(),
            name: self.name.clone(),
            images: self.images.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artist {
    pub id: String,
    pub uri: String,
    pub name: String,
    pub images: Vec<Image>,
    pub followers: Option<u64>,
    pub genres: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playlist {
    pub id: String,
    pub uri: String,
    pub name: String,
    pub description: String,
    pub owner: String,
    pub images: Vec<Image>,
    pub total_tracks: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub display_name: String,
    pub image: Option<String>,
    pub product: Option<String>,
}

impl User {
    pub fn is_premium(&self) -> bool {
        self.product.as_deref() == Some("premium")
    }
}

/// One page of an offset-paged collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub offset: u32,
    pub total: u32,
    /// Raw entries in the server page, including ones we dropped
    /// (episodes, removed tracks). Paging continues from `offset + raw_len`.
    #[serde(default)]
    pub raw_len: u32,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, offset: u32, total: u32) -> Self {
        let raw_len = items.len() as u32;
        Self {
            items,
            offset,
            total,
            raw_len,
        }
    }

    pub fn next_offset(&self) -> Option<u32> {
        let len = (self.raw_len as usize).max(self.items.len());
        super::paging::next_offset(self.offset, len, self.total)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResults {
    pub query: String,
    pub tracks: Vec<Track>,
    pub artists: Vec<Artist>,
    pub albums: Vec<Album>,
    pub playlists: Vec<Playlist>,
}

impl SearchResults {
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
            && self.artists.is_empty()
            && self.albums.is_empty()
            && self.playlists.is_empty()
    }
}

/// Everything the Home screen shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeFeed {
    pub recent_tracks: Vec<Track>,
    pub top_artists: Vec<Artist>,
    pub top_tracks: Vec<Track>,
}

/// A user's library (sidebar contents).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Library {
    pub playlists: Vec<Playlist>,
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    pub liked_total: u32,
}

/// The track list behind a playlist / album / liked songs view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackList {
    pub tracks: Vec<Track>,
    pub total: u32,
    /// Offset of the next server page, if any.
    #[serde(default)]
    pub next: Option<u32>,
}

impl TrackList {
    pub fn from_page(page: Page<Track>) -> Self {
        Self {
            next: page.next_offset(),
            total: page.total,
            tracks: page.items,
        }
    }

    /// Append the next page if it is the one we asked for.
    pub fn append(&mut self, page: Page<Track>) -> bool {
        if self.next != Some(page.offset) {
            return false;
        }
        self.next = page.next_offset();
        self.total = page.total;
        self.tracks.extend(page.items);
        true
    }

    pub fn has_more(&self) -> bool {
        self.next.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistPage {
    pub artist: Artist,
    pub top_tracks: Vec<Track>,
    pub albums: Vec<Album>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32) -> Image {
        Image {
            url: format!("u{w}"),
            width: Some(w),
            height: Some(w),
        }
    }

    #[test]
    fn picks_smallest_sufficient_image() {
        let imgs = vec![img(640), img(64), img(300)];
        assert_eq!(pick_image(&imgs, 200).unwrap().url, "u300");
        assert_eq!(pick_image(&imgs, 10).unwrap().url, "u64");
        assert_eq!(pick_image(&imgs, 1000).unwrap().url, "u640");
        assert!(pick_image(&[], 10).is_none());
    }
}
