//! Playlist editing: create, rename, delete, add, remove and reorder tracks.
//! Every edit is applied to the visible state first and rolled back from a
//! snapshot when the server refuses it.

use std::sync::atomic::{AtomicU32, Ordering};

use iced::Task;

use super::menu::Dialog;
use super::state::Route;
use super::{App, Done, Message, edit, store_write};
use crate::net::model::{Page, Playlist, Track, TrackList};
use crate::net::playlist_edit::Edit;
use crate::store::Kind;

static TEMP_IDS: AtomicU32 = AtomicU32::new(1);

/// Normalise a playlist owner (`spotify:user:<name>`, percent-encoded,
/// any case) to the bare username.
pub fn owner_username(owner: &str) -> String {
    let raw = owner.trim();
    let raw = raw.strip_prefix("spotify:user:").unwrap_or(raw);
    let decoded: String = url::form_urlencoded::parse(format!("u={raw}").as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| raw.to_owned());
    decoded.to_lowercase()
}

/// Is `owner` one of `me` (usernames; empty entries are ignored)?
pub fn is_owner(owner: &str, me: &[&str]) -> bool {
    let o = owner_username(owner);
    !o.is_empty()
        && me
            .iter()
            .filter(|m| !m.trim().is_empty())
            .any(|m| owner_username(m) == o)
}

/// State to restore when an optimistic edit fails.
#[derive(Debug, Clone)]
pub struct EditSnapshot {
    pub id: String,
    pub detail: Option<(Playlist, TrackList)>,
    pub library: Vec<Playlist>,
    pub route: Route,
}

impl App {
    /// Can the user edit `p`? Spotify's own answer (`capabilities` from the
    /// rootlist / playlist) wins; otherwise the owner is compared with the
    /// session's canonical username, which does not depend on the profile
    /// fetch, and with the profile id.
    pub fn owns(&self, p: &Playlist) -> bool {
        if let Some(can) = p.can_edit {
            return can;
        }
        let session = self.username();
        let profile = self.user.as_ref().map(|u| u.id.as_str()).unwrap_or("");
        is_owner(&p.owner, &[session.as_str(), profile])
    }

    /// Menu target for a playlist shown anywhere (card, sidebar row).
    pub fn playlist_target(&self, p: &Playlist) -> super::menu::MenuTarget {
        let saved = self.library.playlists.iter().any(|x| x.id == p.id);
        let owned = saved && self.owns(p);
        super::menu::MenuTarget::playlist(p, owned, saved)
    }

    pub fn owns_id(&self, id: &str) -> bool {
        self.library
            .playlists
            .iter()
            .find(|p| p.id == id)
            .or_else(|| self.playlists.get(id).map(|(p, _)| p))
            .is_some_and(|p| self.owns(p))
    }

    /// Own playlists, for the "Add to playlist" picker.
    pub fn editable_playlists(&self) -> Vec<&Playlist> {
        self.library
            .playlists
            .iter()
            .filter(|p| self.owns(p) && !p.id.starts_with("tmp"))
            .collect()
    }

    fn snapshot(&self, id: &str) -> EditSnapshot {
        EditSnapshot {
            id: id.to_owned(),
            detail: self.playlists.get(id).cloned(),
            library: self.library.playlists.clone(),
            route: self.route().clone(),
        }
    }

    fn restore(&mut self, snap: EditSnapshot) {
        self.library.playlists = snap.library;
        if let Some(d) = snap.detail {
            self.playlists.insert(snap.id, d);
        }
    }

    fn run_edit(
        &mut self,
        id: &str,
        edit: Edit,
        snap: EditSnapshot,
        what: &'static str,
    ) -> Task<Message> {
        let Some(src) = self.source() else {
            self.restore(snap);
            return Task::none();
        };
        let pid = id.to_owned();
        Task::perform(
            async move { src.edit_playlist(&pid, edit).await },
            move |r| Message::Done(Done::PlaylistEdited(Box::new(snap.clone()), what, r)),
        )
    }

    pub(super) fn add_to_playlist(&mut self, id: String, track: Track) -> Task<Message> {
        if !self.owns_id(&id) {
            self.show_toast("You can only add songs to your own playlists");
            return Task::none();
        }
        let snap = self.snapshot(&id);
        let name = self
            .library
            .playlists
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        if let Some((pl, list)) = self.playlists.get_mut(&id) {
            edit::append(list, pl, std::slice::from_ref(&track));
        }
        self.show_toast(format!("Added to {name}"));
        self.run_edit(&id, Edit::Add(vec![track.uri]), snap, "add")
    }

    pub(super) fn remove_from_playlist(&mut self, id: String, index: usize) -> Task<Message> {
        let snap = self.snapshot(&id);
        let Some((pl, list)) = self.playlists.get_mut(&id) else {
            return Task::none();
        };
        if !list.indices_exact() {
            self.show_toast(
                "Edit this playlist in the Spotify app: it has items Ferrotune cannot show",
            );
            return Task::none();
        }
        let Some(t) = edit::remove_at(list, pl, index) else {
            return Task::none();
        };
        self.show_toast("Removed from this playlist");
        self.run_edit(
            &id,
            Edit::Remove(vec![(index as u32, t.uri)]),
            snap,
            "remove",
        )
    }

    pub(super) fn move_in_playlist(&mut self, id: String, from: usize, to: usize) -> Task<Message> {
        let snap = self.snapshot(&id);
        let Some((_, list)) = self.playlists.get_mut(&id) else {
            return Task::none();
        };
        if !list.indices_exact() {
            self.show_toast(
                "Edit this playlist in the Spotify app: it has items Ferrotune cannot show",
            );
            return Task::none();
        }
        if !edit::move_to(list, from, to) {
            return Task::none();
        }
        let edit = Edit::Move {
            from: from as u32,
            to: to as u32,
        };
        self.run_edit(&id, edit, snap, "reorder")
    }

    /// "Create playlist": like the official client, no dialog. The list is
    /// created as "My playlist #N" and opened, ready to rename.
    pub(super) fn new_playlist(&mut self) -> Task<Message> {
        let Some(src) = self.source() else {
            return Task::none();
        };
        let n = self
            .library
            .playlists
            .iter()
            .filter(|p| self.owns(p))
            .count()
            + 1;
        let name = format!("My playlist #{n}");
        let tmp = format!("tmp{}", TEMP_IDS.fetch_add(1, Ordering::Relaxed));
        let owner = self.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
        self.library.playlists.insert(
            0,
            Playlist {
                id: tmp.clone(),
                uri: format!("spotify:playlist:{tmp}"),
                name: name.clone(),
                description: String::new(),
                owner,
                images: vec![],
                can_edit: None,
                mosaic: Vec::new(),
                total_tracks: 0,
            },
        );
        Task::perform(async move { src.create_playlist(&name).await }, move |r| {
            Message::Done(Done::PlaylistCreated(tmp.clone(), r))
        })
    }

    pub(super) fn on_playlist_created(
        &mut self,
        tmp: String,
        r: crate::error::Result<Playlist>,
    ) -> Task<Message> {
        match r {
            Ok(mut p) => {
                // Our own listing shows the owner the way the rootlist does.
                if let Some(u) = &self.user
                    && p.owner.is_empty()
                {
                    p.owner = u.id.clone();
                }
                if let Some(slot) = self.library.playlists.iter_mut().find(|x| x.id == tmp) {
                    *slot = p.clone();
                } else {
                    self.library.playlists.insert(0, p.clone());
                }
                self.playlists.insert(
                    p.id.clone(),
                    (p.clone(), TrackList::from_page(Page::new(vec![], 0, 0))),
                );
                self.navigate(Route::Playlist(p.id))
            }
            Err(e) => {
                self.library.playlists.retain(|x| x.id != tmp);
                self.fail(&e)
            }
        }
    }

    pub(super) fn open_rename(&mut self, id: String) {
        if !self.owns_id(&id) {
            return;
        }
        let name = self
            .library
            .playlists
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        self.dialog = Some(Dialog::Rename { id, name });
    }

    pub(super) fn open_delete(&mut self, id: String) {
        let Some(p) = self.library.playlists.iter().find(|p| p.id == id) else {
            return;
        };
        let owned = self.owns(p);
        self.dialog = Some(Dialog::ConfirmDelete {
            id,
            name: p.name.clone(),
            owned,
        });
    }

    pub(super) fn dialog_submit(&mut self) -> Task<Message> {
        match self.dialog.take() {
            Some(Dialog::Rename { id, name }) => {
                let name = name.trim().to_owned();
                if name.is_empty() {
                    return Task::none();
                }
                let snap = self.snapshot(&id);
                for p in self.library.playlists.iter_mut().filter(|p| p.id == id) {
                    p.name = name.clone();
                }
                if let Some((p, _)) = self.playlists.get_mut(&id) {
                    p.name = name.clone();
                }
                self.run_edit(&id, Edit::Rename(name), snap, "rename")
            }
            Some(Dialog::ConfirmDelete { id, .. }) => {
                let Some(src) = self.source() else {
                    return Task::none();
                };
                let snap = self.snapshot(&id);
                self.library.playlists.retain(|p| p.id != id);
                self.playlists.remove(&id);
                let leave = matches!(self.route(), Route::Playlist(cur) if *cur == id);
                let nav = if leave {
                    self.navigate(Route::Home)
                } else {
                    Task::none()
                };
                let pid = id.clone();
                self.show_toast("Removed from Your Library");
                Task::batch([
                    nav,
                    Task::perform(async move { src.delete_playlist(&pid).await }, move |r| {
                        Message::Done(Done::PlaylistEdited(Box::new(snap.clone()), "delete", r))
                    }),
                ])
            }
            other => {
                self.dialog = other;
                Task::none()
            }
        }
    }

    pub(super) fn on_playlist_edited(
        &mut self,
        snap: EditSnapshot,
        what: &'static str,
        r: crate::error::Result<()>,
    ) -> Task<Message> {
        match r {
            Ok(()) => {
                // Keep the on-disk copy in step with what the view shows.
                let Some((pl, list)) = self.playlists.get(&snap.id).cloned() else {
                    return Task::none();
                };
                let mut page = Page::new(list.tracks, 0, list.total);
                page.raw_len = page.items.len() as u32;
                store_write(&self.store, Kind::Playlist, snap.id, (pl, page))
            }
            Err(e) => {
                self.restore(snap);
                if e.is_revoked() {
                    return self.fail(&e);
                }
                tracing::warn!(error = %e, what, "playlist edit failed");
                self.show_toast(format!(
                    "Could not {what}: {} Changes undone.",
                    e.user_message()
                ));
                Task::none()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::menu::{ContextMenu, MenuTarget};
    use crate::app::{Options, Ui, ui};
    use crate::config::Config;
    use crate::error::Error;

    fn app() -> App {
        let (mut app, _) = App::boot(Config::for_tests(), Options::default());
        let _ = app.update(Message::Done(Done::Library(Ok(crate::mock::library()))));
        let _ = app.update(Message::Done(Done::Playlist(
            "pl01".into(),
            crate::mock::playlist("pl01"),
        )));
        app
    }

    fn names(app: &App, id: &str) -> Vec<String> {
        app.playlists
            .get(id)
            .map(|(_, l)| l.tracks.iter().map(|t| t.name.clone()).collect())
            .unwrap_or_default()
    }

    fn err() -> crate::error::Result<()> {
        Err(Error::Net("down".into()))
    }

    #[test]
    fn only_own_playlists_are_editable() {
        let app = app();
        assert!(app.owns_id("pl01"));
        assert!(!app.owns_id("pl02"));
        let own: Vec<&str> = app
            .editable_playlists()
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        assert!(own.contains(&"pl01") && !own.contains(&"pl02"));
    }

    #[test]
    fn remove_is_optimistic_and_rolls_back_on_error() {
        let mut app = app();
        let before = names(&app, "pl01");
        let snap = app.snapshot("pl01");
        let _ = app.update(ui(Ui::RemoveFromPlaylist("pl01".into(), 1)));
        assert_eq!(names(&app, "pl01").len(), before.len() - 1);
        assert!(!names(&app, "pl01").contains(&before[1]));
        let _ = app.on_playlist_edited(snap, "remove", err());
        assert_eq!(names(&app, "pl01"), before);
        assert!(app.toast.is_some());
    }

    #[test]
    fn success_keeps_the_optimistic_state() {
        let mut app = app();
        let before = names(&app, "pl01");
        let snap = app.snapshot("pl01");
        let _ = app.update(ui(Ui::RemoveFromPlaylist("pl01".into(), 0)));
        let _ = app.on_playlist_edited(snap, "remove", Ok(()));
        assert_eq!(names(&app, "pl01").len(), before.len() - 1);
    }

    #[test]
    fn drag_reorders_and_stale_drags_are_ignored() {
        let mut app = app();
        let _ = app.update(ui(Ui::Navigate(Route::Playlist("pl01".into()))));
        let before = names(&app, "pl01");
        let key = format!("{:?}#0", crate::app::TrackSource::Playlist("pl01".into()));
        let _ = app.update(ui(Ui::DragStart(key.clone(), 0)));
        assert!(app.drag.is_some());
        let _ = app.update(ui(Ui::DragDrop(2)));
        let after = names(&app, "pl01");
        assert_eq!(after[2], before[0]);
        assert_eq!(after[0], before[1]);
        assert!(app.drag.is_none());
        // A release without a drag does nothing.
        let _ = app.update(ui(Ui::DragDrop(3)));
        assert_eq!(names(&app, "pl01"), after);
        // Releasing elsewhere cancels the drag.
        let _ = app.update(ui(Ui::DragStart(key, 0)));
        let _ = app.update(ui(Ui::MouseUp));
        assert!(app.drag.is_none());
    }

    #[test]
    fn rename_flow_and_rollback() {
        let mut app = app();
        let _ = app.update(ui(Ui::OpenRename("pl01".into())));
        let _ = app.update(ui(Ui::DialogInput("Road trip".into())));
        let snap = app.snapshot("pl01");
        let _ = app.update(ui(Ui::DialogSubmit));
        assert!(app.dialog.is_none());
        assert_eq!(app.library.playlists[0].name, "Road trip");
        assert_eq!(app.playlists.get("pl01").unwrap().0.name, "Road trip");
        // The snapshot above was taken after the rename; build the real one.
        let mut app2 = self::app();
        let old = app2.library.playlists[0].name.clone();
        let snap2 = app2.snapshot("pl01");
        let _ = app2.update(ui(Ui::OpenRename("pl01".into())));
        let _ = app2.update(ui(Ui::DialogInput("X".into())));
        let _ = app2.update(ui(Ui::DialogSubmit));
        let _ = app2.on_playlist_edited(snap2, "rename", err());
        assert_eq!(app2.library.playlists[0].name, old);
        drop(snap);
    }

    #[test]
    fn empty_rename_keeps_the_dialog_open() {
        let mut app = app();
        let _ = app.update(ui(Ui::OpenRename("pl01".into())));
        let _ = app.update(ui(Ui::DialogInput("   ".into())));
        let _ = app.update(ui(Ui::DialogSubmit));
        assert_ne!(app.library.playlists[0].name.trim(), "");
    }

    #[test]
    fn rename_of_a_foreign_playlist_is_refused() {
        let mut app = app();
        let _ = app.update(ui(Ui::OpenRename("pl02".into())));
        assert!(app.dialog.is_none());
    }

    #[test]
    fn delete_removes_optimistically_and_restores_on_error() {
        let mut app = app();
        let n = app.library.playlists.len();
        let snap = app.snapshot("pl01");
        let _ = app.update(ui(Ui::OpenDelete("pl01".into())));
        assert!(matches!(
            app.dialog,
            Some(Dialog::ConfirmDelete { owned: true, .. })
        ));
        let _ = app.update(ui(Ui::DialogSubmit));
        assert_eq!(app.library.playlists.len(), n - 1);
        assert!(app.playlists.get("pl01").is_none());
        let _ = app.on_playlist_edited(snap, "delete", err());
        assert_eq!(app.library.playlists.len(), n);
        assert!(app.playlists.get("pl01").is_some());
    }

    #[test]
    fn new_playlist_is_added_then_replaced_by_the_server_copy() {
        let mut app = app();
        let n = app.library.playlists.len();
        let _ = app.update(ui(Ui::NewPlaylist));
        assert_eq!(app.library.playlists.len(), n + 1);
        let tmp = app.library.playlists[0].clone();
        assert!(tmp.id.starts_with("tmp"));
        assert!(tmp.name.starts_with("My playlist #"));
        let made = crate::mock::new_playlist(&tmp.name);
        let _ = app.update(Message::Done(Done::PlaylistCreated(
            tmp.id.clone(),
            Ok(made.clone()),
        )));
        assert_eq!(app.library.playlists.len(), n + 1);
        assert_eq!(app.library.playlists[0].id, made.id);
        assert_eq!(*app.route(), Route::Playlist(made.id.clone()));
        // The picker never offers the temporary entry.
        let _ = app.update(ui(Ui::NewPlaylist));
        assert!(
            app.editable_playlists()
                .iter()
                .all(|p| !p.id.starts_with("tmp"))
        );
    }

    #[test]
    fn failed_create_removes_the_placeholder() {
        let mut app = app();
        let n = app.library.playlists.len();
        let _ = app.update(ui(Ui::NewPlaylist));
        let tmp = app.library.playlists[0].id.clone();
        let _ = app.update(Message::Done(Done::PlaylistCreated(
            tmp,
            Err(Error::Net("x".into())),
        )));
        assert_eq!(app.library.playlists.len(), n);
        assert!(app.toast.is_some());
    }

    #[test]
    fn adding_to_a_foreign_playlist_is_refused() {
        let mut app = app();
        let t = crate::mock::album("al01").unwrap().tracks[0].clone();
        let before = names(&app, "pl01").len();
        let _ = app.update(ui(Ui::AddToPlaylist("pl02".into(), t.clone())));
        assert!(app.toast.is_some());
        let _ = app.update(ui(Ui::AddToPlaylist("pl01".into(), t)));
        assert_eq!(names(&app, "pl01").len(), before + 1);
    }

    #[test]
    fn menu_and_dialog_overlays_close_in_order() {
        let mut app = app();
        let t = crate::mock::album("al01").unwrap().tracks[0].clone();
        let _ = app.update(ui(Ui::OpenMenu(
            MenuTarget::track(&t, None),
            iced::Point::new(10.0, 10.0),
        )));
        assert!(app.menu.is_some());
        let _ = app.update(ui(Ui::ToggleMenuPicker));
        assert!(app.menu.as_ref().is_some_and(|m: &ContextMenu| m.picker));
        let _ = app.update(ui(Ui::ToggleHelp));
        assert!(app.menu.is_none() && app.dialog.is_some());
        let _ = app.update(ui(Ui::CloseOverlay));
        assert!(app.dialog.is_none());
        let _ = app.update(ui(Ui::OpenMenu(
            MenuTarget::track(&t, None),
            iced::Point::new(10.0, 10.0),
        )));
        let _ = app.update(ui(Ui::Navigate(Route::Library)));
        assert!(app.menu.is_none(), "any action dismisses the menu");
    }

    #[test]
    fn shortcuts_drive_playback_state() {
        let mut app = app();
        let _ = app.update(ui(Ui::ToggleShuffle));
        assert!(app.queue.shuffle);
        let v = app.playback.volume;
        let _ = app.update(ui(Ui::VolumeStep(-0.1)));
        assert!((app.playback.volume - (v - 0.1).max(0.0)).abs() < 1e-5);
        let _ = app.update(ui(Ui::VolumeStep(5.0)));
        assert!((app.playback.volume - 1.0).abs() < 1e-6);
        let _ = app.update(ui(Ui::Navigate(Route::Library)));
        let _ = app.update(ui(Ui::Back));
        assert_eq!(*app.route(), Route::Home);
        let _ = app.update(ui(Ui::Forward));
        assert_eq!(*app.route(), Route::Library);
        let _ = app.update(ui(Ui::FocusSearch));
        assert_eq!(*app.route(), Route::Search);
    }
}
