//! Pure reducers for optimistic playlist edits (unit-tested). The app applies
//! them immediately and restores a snapshot if the server refuses.

use crate::net::model::{Playlist, Track, TrackList};

impl TrackList {
    /// Do positions in `tracks` equal the server's positions? False when
    /// loaded entries were dropped (episodes, removed tracks), in which case
    /// index-based edits would hit the wrong item.
    pub fn indices_exact(&self) -> bool {
        let loaded_raw = self.next.unwrap_or(self.total);
        loaded_raw as usize == self.tracks.len()
    }
}

/// Remove the track at `index`. Returns it.
pub fn remove_at(list: &mut TrackList, pl: &mut Playlist, index: usize) -> Option<Track> {
    if index >= list.tracks.len() {
        return None;
    }
    let t = list.tracks.remove(index);
    list.total = list.total.saturating_sub(1);
    list.next = list.next.map(|n| n.saturating_sub(1));
    pl.total_tracks = pl.total_tracks.saturating_sub(1);
    Some(t)
}

/// Move the track at `from` so it ends up at `to`.
pub fn move_to(list: &mut TrackList, from: usize, to: usize) -> bool {
    let n = list.tracks.len();
    if from >= n || to >= n || from == to {
        return false;
    }
    let t = list.tracks.remove(from);
    list.tracks.insert(to, t);
    true
}

/// Append tracks. Only meaningful when the whole list is loaded; otherwise
/// the server appends at the end and the view catches up on the next load.
pub fn append(list: &mut TrackList, pl: &mut Playlist, tracks: &[Track]) {
    let n = u32::try_from(tracks.len()).unwrap_or(0);
    list.total += n;
    pl.total_tracks += n;
    if !list.has_more() {
        list.tracks.extend(tracks.iter().cloned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str) -> Track {
        Track {
            id: id.into(),
            uri: format!("spotify:track:{id}"),
            name: id.into(),
            artists: vec![],
            album: None,
            duration_ms: 1,
            explicit: false,
            playable: true,
            added_at: None,
        }
    }

    fn pl(n: u32) -> Playlist {
        Playlist {
            id: "p".into(),
            uri: "spotify:playlist:p".into(),
            name: "P".into(),
            description: String::new(),
            owner: "me".into(),
            images: vec![],
            total_tracks: n,
        }
    }

    fn list(ids: &[&str]) -> TrackList {
        TrackList {
            tracks: ids.iter().map(|i| t(i)).collect(),
            total: ids.len() as u32,
            next: None,
        }
    }

    fn ids(l: &TrackList) -> Vec<&str> {
        l.tracks.iter().map(|t| t.id.as_str()).collect()
    }

    #[test]
    fn remove_updates_counts() {
        let mut l = list(&["a", "b", "c"]);
        let mut p = pl(3);
        assert_eq!(remove_at(&mut l, &mut p, 1).unwrap().id, "b");
        assert_eq!(ids(&l), ["a", "c"]);
        assert_eq!((l.total, p.total_tracks), (2, 2));
        assert!(remove_at(&mut l, &mut p, 5).is_none());
    }

    #[test]
    fn move_down_and_up() {
        let mut l = list(&["a", "b", "c", "d"]);
        assert!(move_to(&mut l, 0, 2));
        assert_eq!(ids(&l), ["b", "c", "a", "d"]);
        assert!(move_to(&mut l, 3, 0));
        assert_eq!(ids(&l), ["d", "b", "c", "a"]);
        assert!(!move_to(&mut l, 1, 1));
        assert!(!move_to(&mut l, 1, 9));
    }

    #[test]
    fn append_only_extends_a_fully_loaded_list() {
        let mut l = list(&["a"]);
        let mut p = pl(1);
        append(&mut l, &mut p, &[t("b")]);
        assert_eq!(ids(&l), ["a", "b"]);
        assert_eq!(p.total_tracks, 2);
        let mut partial = TrackList {
            tracks: vec![t("a")],
            total: 5,
            next: Some(1),
        };
        let mut p = pl(5);
        append(&mut partial, &mut p, &[t("z")]);
        assert_eq!(partial.tracks.len(), 1);
        assert_eq!(partial.total, 6);
    }

    #[test]
    fn index_exactness_detects_dropped_entries() {
        assert!(list(&["a", "b"]).indices_exact());
        let dropped = TrackList {
            tracks: vec![t("a")],
            total: 2,
            next: None,
        };
        assert!(!dropped.indices_exact());
        let partial = TrackList {
            tracks: vec![t("a"), t("b")],
            total: 9,
            next: Some(2),
        };
        assert!(partial.indices_exact());
    }
}
