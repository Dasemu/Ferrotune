//! Context menus and dialogs: the pure model (what the menu offers for a
//! target) so it can be unit-tested without iced.

use iced::Point;

use super::message::Ui;
use crate::net::model::{Album, Artist, ArtistRef, Playlist, Track};

/// What was right-clicked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuTarget {
    Track {
        track: Track,
        /// `(playlist id, index)` when the row is in a playlist the user owns
        /// and the index is exact (so "Remove from this playlist" applies).
        removable_from: Option<(String, usize)>,
    },
    Album {
        id: String,
        artists: Vec<ArtistRef>,
    },
    Artist {
        id: String,
    },
    Playlist {
        id: String,
        name: String,
        owned: bool,
        /// In the user's library (so it can be removed from it).
        saved: bool,
    },
}

impl MenuTarget {
    pub fn track(track: &Track, removable_from: Option<(String, usize)>) -> Self {
        Self::Track {
            track: track.clone(),
            removable_from,
        }
    }

    pub fn album(a: &Album) -> Self {
        Self::Album {
            id: a.id.clone(),
            artists: a.artists.clone(),
        }
    }

    pub fn artist(a: &Artist) -> Self {
        Self::Artist { id: a.id.clone() }
    }

    pub fn playlist(p: &Playlist, owned: bool, saved: bool) -> Self {
        Self::Playlist {
            id: p.id.clone(),
            name: p.name.clone(),
            owned,
            saved,
        }
    }

    /// `https://open.spotify.com/...` link for "Copy link".
    pub fn link(&self) -> String {
        match self {
            Self::Track { track, .. } => format!("https://open.spotify.com/track/{}", track.id),
            Self::Album { id, .. } => format!("https://open.spotify.com/album/{id}"),
            Self::Artist { id } => format!("https://open.spotify.com/artist/{id}"),
            Self::Playlist { id, .. } => format!("https://open.spotify.com/playlist/{id}"),
        }
    }
}

/// One selectable entry.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    /// Runs a UI message and closes the menu.
    Action {
        label: String,
        msg: EntryMsg,
    },
    /// Expands the playlist picker ("Add to playlist ▸").
    AddToPlaylist,
    Separator,
}

/// The message an entry sends (a plain enum so entries are comparable).
#[derive(Debug, Clone, PartialEq)]
pub enum EntryMsg {
    Enqueue(Box<Track>),
    GoToArtist(String),
    GoToAlbum(String),
    ToggleLike(Box<Track>),
    RemoveFromPlaylist(String, usize),
    CopyLink(String),
    RenamePlaylist(String),
    DeletePlaylist(String),
    PlayPlaylist(String),
}

/// An open context menu.
#[derive(Debug, Clone)]
pub struct ContextMenu {
    pub target: MenuTarget,
    /// Window coordinates of the click.
    pub at: Point,
    /// "Add to playlist" picker expanded.
    pub picker: bool,
}

/// The entries for `target`. `liked` is the heart state for a track.
pub fn entries(target: &MenuTarget, liked: bool) -> Vec<Entry> {
    let act = |label: &str, msg| Entry::Action {
        label: label.to_owned(),
        msg,
    };
    let mut v = Vec::new();
    match target {
        MenuTarget::Track {
            track,
            removable_from,
        } => {
            if track.playable {
                v.push(act(
                    "Add to queue",
                    EntryMsg::Enqueue(Box::new(track.clone())),
                ));
                v.push(Entry::Separator);
            }
            for a in track.artists.iter().take(3) {
                let label = if track.artists.len() > 1 {
                    format!("Go to artist: {}", a.name)
                } else {
                    "Go to artist".to_owned()
                };
                v.push(act(&label, EntryMsg::GoToArtist(a.id.clone())));
            }
            if let Some(al) = &track.album {
                v.push(act("Go to album", EntryMsg::GoToAlbum(al.id.clone())));
            }
            v.push(Entry::Separator);
            v.push(act(
                if liked {
                    "Remove from your Liked Songs"
                } else {
                    "Save to your Liked Songs"
                },
                EntryMsg::ToggleLike(Box::new(track.clone())),
            ));
            v.push(Entry::AddToPlaylist);
            if let Some((pl, i)) = removable_from {
                v.push(act(
                    "Remove from this playlist",
                    EntryMsg::RemoveFromPlaylist(pl.clone(), *i),
                ));
            }
            v.push(Entry::Separator);
            v.push(act("Copy song link", EntryMsg::CopyLink(target.link())));
        }
        MenuTarget::Album { id, artists } => {
            for a in artists.iter().take(3) {
                let label = if artists.len() > 1 {
                    format!("Go to artist: {}", a.name)
                } else {
                    "Go to artist".to_owned()
                };
                v.push(act(&label, EntryMsg::GoToArtist(a.id.clone())));
            }
            v.push(act("Go to album", EntryMsg::GoToAlbum(id.clone())));
            v.push(Entry::Separator);
            v.push(act("Copy album link", EntryMsg::CopyLink(target.link())));
        }
        MenuTarget::Artist { id } => {
            v.push(act("Go to artist", EntryMsg::GoToArtist(id.clone())));
            v.push(Entry::Separator);
            v.push(act("Copy artist link", EntryMsg::CopyLink(target.link())));
        }
        MenuTarget::Playlist {
            id, owned, saved, ..
        } => {
            v.push(act("Play", EntryMsg::PlayPlaylist(id.clone())));
            v.push(Entry::Separator);
            if *owned {
                v.push(act("Rename", EntryMsg::RenamePlaylist(id.clone())));
                v.push(act("Delete", EntryMsg::DeletePlaylist(id.clone())));
            } else if *saved {
                v.push(act(
                    "Remove from Your Library",
                    EntryMsg::DeletePlaylist(id.clone()),
                ));
            }
            v.push(Entry::Separator);
            v.push(act("Copy playlist link", EntryMsg::CopyLink(target.link())));
        }
    }
    v
}

/// A modal dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    Shortcuts,
    Rename {
        id: String,
        name: String,
    },
    ConfirmDelete {
        id: String,
        name: String,
        owned: bool,
    },
}

impl Entry {
    /// The UI message an action entry sends.
    pub fn message(msg: &EntryMsg) -> Ui {
        match msg {
            EntryMsg::Enqueue(t) => Ui::Enqueue((**t).clone()),
            EntryMsg::GoToArtist(id) => Ui::Navigate(super::state::Route::Artist(id.clone())),
            EntryMsg::GoToAlbum(id) => Ui::Navigate(super::state::Route::Album(id.clone())),
            EntryMsg::ToggleLike(t) => Ui::ToggleLike((**t).clone()),
            EntryMsg::RemoveFromPlaylist(p, i) => Ui::RemoveFromPlaylist(p.clone(), *i),
            EntryMsg::CopyLink(l) => Ui::CopyLink(l.clone()),
            EntryMsg::RenamePlaylist(id) => Ui::OpenRename(id.clone()),
            EntryMsg::DeletePlaylist(id) => Ui::OpenDelete(id.clone()),
            EntryMsg::PlayPlaylist(id) => {
                Ui::PlayFrom(super::message::TrackSource::Playlist(id.clone()), None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::model::{AlbumRef, ArtistRef};

    fn track(playable: bool, artists: usize) -> Track {
        Track {
            id: "t1".into(),
            uri: "spotify:track:t1".into(),
            name: "Song".into(),
            artists: (0..artists)
                .map(|i| ArtistRef {
                    id: format!("a{i}"),
                    name: format!("Artist {i}"),
                })
                .collect(),
            album: Some(AlbumRef {
                id: "al1".into(),
                name: "Album".into(),
                images: vec![],
            }),
            duration_ms: 1000,
            explicit: false,
            playable,
            added_at: None,
        }
    }

    fn labels(v: &[Entry]) -> Vec<String> {
        v.iter()
            .filter_map(|e| match e {
                Entry::Action { label, .. } => Some(label.clone()),
                Entry::AddToPlaylist => Some("Add to playlist".into()),
                Entry::Separator => None,
            })
            .collect()
    }

    #[test]
    fn track_menu_has_the_basics() {
        let t = MenuTarget::track(&track(true, 1), None);
        let l = labels(&entries(&t, false));
        assert_eq!(
            l,
            [
                "Add to queue",
                "Go to artist",
                "Go to album",
                "Save to your Liked Songs",
                "Add to playlist",
                "Copy song link"
            ]
        );
        let liked = labels(&entries(&t, true));
        assert!(liked.contains(&"Remove from your Liked Songs".to_string()));
    }

    #[test]
    fn remove_from_playlist_only_for_removable_rows() {
        let plain = MenuTarget::track(&track(true, 1), None);
        assert!(!labels(&entries(&plain, false)).contains(&"Remove from this playlist".into()));
        let own = MenuTarget::track(&track(true, 1), Some(("pl".into(), 3)));
        let e = entries(&own, false);
        assert!(e.iter().any(|e| matches!(
            e,
            Entry::Action { msg: EntryMsg::RemoveFromPlaylist(p, 3), .. } if p == "pl"
        )));
    }

    #[test]
    fn unplayable_track_cannot_be_queued_and_many_artists_are_listed() {
        let t = MenuTarget::track(&track(false, 2), None);
        let l = labels(&entries(&t, false));
        assert!(!l.contains(&"Add to queue".to_string()));
        assert!(l.contains(&"Go to artist: Artist 0".to_string()));
        assert!(l.contains(&"Go to artist: Artist 1".to_string()));
    }

    #[test]
    fn playlist_menu_depends_on_ownership() {
        let own = MenuTarget::Playlist {
            id: "p".into(),
            name: "P".into(),
            owned: true,
            saved: true,
        };
        let l = labels(&entries(&own, false));
        assert!(l.contains(&"Rename".to_string()) && l.contains(&"Delete".to_string()));
        let other = MenuTarget::Playlist {
            id: "p".into(),
            name: "P".into(),
            owned: false,
            saved: true,
        };
        let l = labels(&entries(&other, false));
        assert!(!l.contains(&"Rename".to_string()));
        assert!(l.contains(&"Remove from Your Library".to_string()));
        let unsaved = MenuTarget::Playlist {
            id: "p".into(),
            name: "P".into(),
            owned: false,
            saved: false,
        };
        let l = labels(&entries(&unsaved, false));
        assert_eq!(l, ["Play", "Copy playlist link"]);
    }

    #[test]
    fn links_use_open_spotify_urls() {
        let t = MenuTarget::track(&track(true, 1), None);
        assert_eq!(t.link(), "https://open.spotify.com/track/t1");
        assert_eq!(
            MenuTarget::Artist { id: "x".into() }.link(),
            "https://open.spotify.com/artist/x"
        );
    }
}

/// Top-left corner for a menu of `menu` size opened at `at`, kept inside the
/// window (flipped up/left when it would overflow).
pub fn place(at: Point, window: iced::Size, menu: iced::Size) -> Point {
    const MARGIN: f32 = 8.0;
    let x = if at.x + menu.width + MARGIN > window.width {
        (at.x - menu.width).max(MARGIN)
    } else {
        at.x
    };
    let y = if at.y + menu.height + MARGIN > window.height {
        (window.height - menu.height - MARGIN).max(MARGIN)
    } else {
        at.y
    };
    Point::new(x, y)
}

#[cfg(test)]
mod place_tests {
    use super::*;
    use iced::Size;

    #[test]
    fn menu_stays_inside_the_window() {
        let w = Size::new(800.0, 600.0);
        let m = Size::new(240.0, 200.0);
        assert_eq!(
            place(Point::new(100.0, 100.0), w, m),
            Point::new(100.0, 100.0)
        );
        let p = place(Point::new(790.0, 590.0), w, m);
        assert!(p.x + m.width <= w.width && p.y + m.height <= w.height);
        assert_eq!(p.x, 550.0);
        // Larger than the window: pinned to the margin, never negative.
        let p = place(Point::new(5.0, 5.0), Size::new(100.0, 100.0), m);
        assert!(p.x >= 0.0 && p.y >= 0.0);
    }
}
