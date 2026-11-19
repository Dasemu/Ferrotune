//! Protocol clients: Spotify Web API (`webapi`), librespot playback
//! (`playback`), album art (`images`) and MPRIS (`mpris`).
//!
//! Never touches UI types.

pub mod images;
pub mod model;
pub mod mpris;
pub mod paging;
pub mod playback;
pub mod types;
pub mod webapi;

use std::time::Duration;

use model::{Album, ArtistPage, HomeFeed, Library, Page, Playlist, SearchResults, Track, User};

use crate::error::Result;
use crate::mock;

/// Where library data comes from: the real Web API or the mock fixtures.
#[derive(Clone)]
pub enum DataSource {
    Web(webapi::WebApi),
    Mock,
}

impl std::fmt::Debug for DataSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Web(_) => "DataSource::Web",
            Self::Mock => "DataSource::Mock",
        })
    }
}

/// Simulated latency for mock calls so loading states are exercised.
const MOCK_LATENCY: Duration = Duration::from_millis(30);

impl DataSource {
    pub async fn me(&self) -> Result<User> {
        match self {
            Self::Web(w) => w.me().await,
            Self::Mock => Ok(mock::user()),
        }
    }

    pub async fn library(&self) -> Result<Library> {
        match self {
            Self::Web(w) => w.library().await,
            Self::Mock => {
                tokio::time::sleep(MOCK_LATENCY).await;
                Ok(mock::library())
            }
        }
    }

    pub async fn home(&self) -> Result<HomeFeed> {
        match self {
            Self::Web(w) => w.home().await,
            Self::Mock => Ok(mock::home()),
        }
    }

    pub async fn playlist(&self, id: &str) -> Result<(Playlist, Page<Track>)> {
        match self {
            Self::Web(w) => w.playlist(id).await,
            Self::Mock => mock::playlist(id),
        }
    }

    pub async fn playlist_tracks(&self, id: &str, offset: u32) -> Result<Page<Track>> {
        match self {
            Self::Web(w) => w.playlist_tracks(id, offset).await,
            Self::Mock => Ok(Page::new(vec![], offset, offset)),
        }
    }

    pub async fn saved_tracks(&self, offset: u32) -> Result<Page<Track>> {
        match self {
            Self::Web(w) => w.saved_tracks(offset).await,
            Self::Mock => Ok(mock::liked(offset)),
        }
    }

    pub async fn album(&self, id: &str) -> Result<Album> {
        match self {
            Self::Web(w) => w.album(id).await,
            Self::Mock => mock::album(id),
        }
    }

    pub async fn artist_page(&self, id: &str) -> Result<ArtistPage> {
        match self {
            Self::Web(w) => w.artist_page(id).await,
            Self::Mock => mock::artist_page(id),
        }
    }

    pub async fn search(&self, query: &str) -> Result<SearchResults> {
        match self {
            Self::Web(w) => w.search(query, 10).await,
            Self::Mock => {
                tokio::time::sleep(MOCK_LATENCY).await;
                Ok(mock::search(query))
            }
        }
    }

    pub async fn set_saved(&self, ids: Vec<String>, saved: bool) -> Result<()> {
        match self {
            Self::Web(w) => w.set_saved(&ids, saved).await,
            Self::Mock => Ok(()),
        }
    }

    pub async fn contains_saved(&self, ids: Vec<String>) -> Result<Vec<bool>> {
        match self {
            Self::Web(w) => w.contains_saved_tracks(&ids).await,
            Self::Mock => Ok(ids.iter().map(|id| mock::is_liked(id)).collect()),
        }
    }

    pub async fn image(&self, url: String, max_px: u32) -> Result<images::DecodedImage> {
        match self {
            Self::Web(_) => {
                // A plain client: image CDN URLs need no auth header.
                static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
                let http = HTTP.get_or_init(webapi::http_client);
                images::fetch(http, &url, max_px).await
            }
            Self::Mock => Ok(mock::image(&url, max_px)),
        }
    }
}
