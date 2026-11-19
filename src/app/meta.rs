//! Lazily completed metadata: playlist owner display names (user-profile
//! view, cached in SQLite by the HTTP cache) and playlists whose rootlist
//! entry came without a name or picture (personalised and Spotify-made
//! lists), which are filled from the playlist itself, with a 2x2 mosaic of
//! the first tracks' covers when there is no picture at all.

use std::collections::HashSet;

use iced::Task;

use super::{App, Done, Message, store_write};
use crate::net::model::{Card, Image, Playlist, Track, pick_image};
use crate::store::Kind;

/// Owner names resolved per batch.
const OWNER_BATCH: usize = 20;
/// Playlists completed per batch.
const FILL_BATCH: usize = 12;

/// Is `s` a Spotify username (not an already resolved display name)?
pub fn looks_like_username(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Display label for a playlist owner. `spotify` is always "Spotify".
pub fn owner_label(owner: &str, me: Option<(&str, &str)>, resolved: Option<&str>) -> String {
    let o = owner.trim();
    if o.eq_ignore_ascii_case("spotify") {
        return "Spotify".into();
    }
    if let Some((id, name)) = me
        && super::playlists::is_owner(o, &[id])
        && !name.is_empty()
    {
        return name.to_owned();
    }
    resolved.unwrap_or(o).to_owned()
}

/// Up to four distinct album covers of the first tracks, the way the
/// official client builds a playlist mosaic. Fewer than four: no mosaic.
pub fn mosaic_from(tracks: &[Track]) -> Vec<String> {
    let mut seen = HashSet::new();
    let urls: Vec<String> = tracks
        .iter()
        .filter_map(|t| pick_image(t.images(), 300))
        .map(|i| i.url.clone())
        .filter(|u| seen.insert(u.clone()))
        .take(4)
        .collect();
    if urls.len() == 4 { urls } else { Vec::new() }
}

/// Does `p` still need its own metadata?
pub fn needs_fill(p: &Playlist) -> bool {
    !p.id.starts_with("tmp")
        && (p.name.trim().is_empty() || (p.images.is_empty() && p.mosaic.is_empty()))
}

/// Complete `target` from the full playlist `header` and its first tracks.
/// Returns whether anything changed.
pub fn fill_playlist(target: &mut Playlist, header: &Playlist, tracks: &[Track]) -> bool {
    let before = target.clone();
    if target.name.trim().is_empty() {
        target.name.clone_from(&header.name);
    }
    if target.description.is_empty() {
        target.description.clone_from(&header.description);
    }
    if target.owner.is_empty() {
        target.owner.clone_from(&header.owner);
    }
    if target.can_edit.is_none() {
        target.can_edit = header.can_edit;
    }
    if target.images.is_empty() {
        target.images.clone_from(&header.images);
    }
    if target.images.is_empty() && target.mosaic.is_empty() {
        target.mosaic = mosaic_from(tracks);
        if target.mosaic.is_empty()
            && let Some(i) = tracks.iter().find_map(|t| pick_image(t.images(), 300))
        {
            target.images = vec![i.clone()];
        }
    }
    *target != before
}

/// Mosaic tiles as images (for the image loader).
pub fn mosaic_images(p: &Playlist) -> Vec<Image> {
    p.mosaic
        .iter()
        .map(|u| Image {
            url: u.clone(),
            width: Some(300),
            height: Some(300),
        })
        .collect()
}

impl App {
    /// Owner label for a playlist (display name, "Spotify", or the id).
    pub fn owner_label(&self, owner: &str) -> String {
        let me = self
            .user
            .as_ref()
            .map(|u| (u.id.as_str(), u.display_name.as_str()));
        let key = super::playlists::owner_username(owner);
        owner_label(owner, me, self.owner_names.get(&key).map(String::as_str))
    }

    fn visible_playlists(&self) -> impl Iterator<Item = &Playlist> {
        let home = self.home.iter().flat_map(|h| {
            h.shelves.iter().flat_map(|s| {
                s.items.iter().filter_map(|c| match c {
                    Card::Playlist(p) => Some(p),
                    _ => None,
                })
            })
        });
        let search = self.search.results.iter().flat_map(|r| r.playlists.iter());
        let details = self.playlists.values().map(|(p, _)| p);
        self.library
            .playlists
            .iter()
            .chain(home)
            .chain(search)
            .chain(details)
    }

    /// Resolve owner usernames we have not seen yet (one small batch).
    pub(super) fn request_owner_names(&mut self) -> Task<Message> {
        let Some(src) = self.source.clone() else {
            return Task::none();
        };
        let me = self.username();
        let mut batch: Vec<String> = Vec::new();
        for p in self.visible_playlists() {
            let key = super::playlists::owner_username(&p.owner);
            if batch.len() >= OWNER_BATCH {
                break;
            }
            if !looks_like_username(&key)
                || key == "spotify"
                || super::playlists::is_owner(&key, &[me.as_str()])
                || self.owner_names.contains_key(&key)
                || self.owner_requested.contains(&key)
                || batch.contains(&key)
            {
                continue;
            }
            batch.push(key);
        }
        if batch.is_empty() {
            return Task::none();
        }
        self.owner_requested.extend(batch.iter().cloned());
        Task::perform(
            async move {
                let mut out = Vec::new();
                for u in batch {
                    match src.profile(u.clone(), 0).await {
                        Ok(p) if p.name != u => out.push((u, p.name)),
                        Ok(_) => {}
                        Err(e) => tracing::debug!(error = %e, "owner name not resolved"),
                    }
                }
                out
            },
            |names| Message::Done(Done::OwnerNames(names)),
        )
    }

    pub(super) fn on_owner_names(&mut self, names: Vec<(String, String)>) -> Task<Message> {
        self.owner_names.extend(names);
        Task::none()
    }

    /// Fetch playlists that came without a name or picture.
    pub(super) fn request_playlist_fills(&mut self) -> Task<Message> {
        let Some(src) = self.source.clone() else {
            return Task::none();
        };
        let mut batch: Vec<String> = Vec::new();
        for p in self.visible_playlists() {
            if batch.len() >= FILL_BATCH {
                break;
            }
            if needs_fill(p)
                && !self.fill_requested.contains(&p.id)
                && !batch.contains(&p.id)
                && p.id.bytes().all(|b| b.is_ascii_alphanumeric())
            {
                batch.push(p.id.clone());
            }
        }
        if batch.is_empty() {
            return Task::none();
        }
        self.fill_requested.extend(batch.iter().cloned());
        Task::perform(
            async move {
                let mut out = Vec::new();
                for id in batch {
                    match src.playlist(&id).await {
                        Ok((header, page)) => {
                            let tracks: Vec<Track> = page.items.into_iter().take(12).collect();
                            out.push((id, header, tracks));
                        }
                        Err(e) => tracing::debug!(error = %e, "playlist metadata not filled"),
                    }
                }
                out
            },
            |v| Message::Done(Done::PlaylistFill(v)),
        )
    }

    /// Apply fetched playlist metadata everywhere the playlist is shown.
    pub(super) fn apply_fill(&mut self, id: &str, header: &Playlist, tracks: &[Track]) -> bool {
        let mut lib_changed = false;
        for p in self.library.playlists.iter_mut().filter(|p| p.id == id) {
            lib_changed |= fill_playlist(p, header, tracks);
        }
        if let Some(h) = self.home.as_mut() {
            for s in &mut h.shelves {
                for c in &mut s.items {
                    if let Card::Playlist(p) = c
                        && p.id == id
                    {
                        fill_playlist(p, header, tracks);
                    }
                }
            }
        }
        lib_changed
    }

    pub(super) fn on_playlist_fill(
        &mut self,
        v: Vec<(String, Playlist, Vec<Track>)>,
    ) -> Task<Message> {
        let mut lib_changed = false;
        for (id, header, tracks) in &v {
            lib_changed |= self.apply_fill(id, header, tracks);
        }
        let mut tasks = Vec::new();
        if lib_changed && !self.is_mock() {
            tasks.push(store_write(
                &self.store,
                Kind::Library,
                "me".into(),
                self.library.clone(),
            ));
        }
        if let Some(h) = self.home.clone()
            && !self.is_mock()
        {
            tasks.push(store_write(&self.store, Kind::Home, "me".into(), h));
        }
        Task::batch(tasks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::model::AlbumRef;

    fn track(i: usize, cover: &str) -> Track {
        Track {
            id: format!("t{i}"),
            uri: format!("spotify:track:t{i}"),
            name: String::new(),
            artists: vec![],
            album: Some(AlbumRef {
                id: String::new(),
                name: String::new(),
                images: vec![Image {
                    url: cover.into(),
                    width: Some(300),
                    height: Some(300),
                }],
            }),
            duration_ms: 1,
            explicit: false,
            playable: true,
            added_at: None,
        }
    }

    fn bare(id: &str) -> Playlist {
        Playlist {
            id: id.into(),
            uri: format!("spotify:playlist:{id}"),
            name: String::new(),
            description: String::new(),
            owner: "spotify".into(),
            images: vec![],
            total_tracks: 0,
            can_edit: None,
            mosaic: vec![],
        }
    }

    #[test]
    fn owner_labels() {
        let me = Some(("0d2xmxvdik4b9wpltcoke", "Ana"));
        assert_eq!(owner_label("spotify", me, None), "Spotify");
        assert_eq!(owner_label("Spotify", me, None), "Spotify");
        assert_eq!(owner_label("0d2xmxvdik4b9wpltcoke", me, None), "Ana");
        assert_eq!(
            owner_label("spotify:user:0D2XMXVDIK4B9WPLTCOKE", me, None),
            "Ana"
        );
        assert_eq!(owner_label("bob99", me, Some("Bob")), "Bob");
        assert_eq!(owner_label("bob99", me, None), "bob99");
        assert!(looks_like_username("0d2xmxvdik4b9wpltcoke"));
        assert!(looks_like_username("john.doe_1"));
        assert!(!looks_like_username("John Smith"));
        assert!(!looks_like_username(""));
    }

    #[test]
    fn fill_uses_header_then_mosaic_then_first_cover() {
        let covers = ["a", "b", "a", "c", "d", "e"];
        let tracks: Vec<Track> = covers
            .iter()
            .enumerate()
            .map(|(i, c)| track(i, c))
            .collect();
        assert_eq!(
            mosaic_from(&tracks),
            ["a", "b", "c", "d"],
            "distinct, in order"
        );
        assert!(
            mosaic_from(&tracks[..3]).is_empty(),
            "fewer than 4 covers: no mosaic"
        );

        let mut header = bare("x");
        header.name = "Daily Mix 1".into();
        header.can_edit = Some(false);
        let mut p = bare("x");
        assert!(needs_fill(&p));
        assert!(fill_playlist(&mut p, &header, &tracks));
        assert_eq!(p.name, "Daily Mix 1");
        assert_eq!(p.mosaic.len(), 4);
        assert_eq!(p.can_edit, Some(false));
        assert!(!needs_fill(&p));
        assert!(!fill_playlist(&mut p, &header, &tracks), "idempotent");

        // The header's own picture wins over a mosaic.
        let mut pictured = bare("y");
        let mut h2 = header.clone();
        h2.images = vec![track(0, "pic").album.unwrap().images[0].clone()];
        fill_playlist(&mut pictured, &h2, &tracks);
        assert!(pictured.mosaic.is_empty());
        assert_eq!(pictured.images[0].url, "pic");

        // Two covers only: the first one becomes the picture.
        let mut few = bare("z");
        fill_playlist(&mut few, &header, &tracks[..2]);
        assert_eq!(few.images[0].url, "a");
        // Existing names are never overwritten.
        let mut named = bare("n");
        named.name = "Mine".into();
        fill_playlist(&mut named, &header, &tracks);
        assert_eq!(named.name, "Mine");
    }
}
