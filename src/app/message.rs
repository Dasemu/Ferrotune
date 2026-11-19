//! The `Message` enum: UI events, net events, task completions.

use std::time::Instant;

use super::state::{LibraryFilter, Route, SearchFilter};
use crate::error::Result;
use crate::net::DataSource;
use crate::net::images::DecodedImage;
use crate::net::model::{
    Album, ArtistPage, HomeFeed, Library, Page, Playlist, SearchResults, Track, User,
};
use crate::net::mpris::MediaEvent;
use crate::net::playback::{PlaybackEvent, SpotifySession};

/// A list of tracks the user can start playing from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackSource {
    Playlist(String),
    Album(String),
    Liked,
    ArtistTop(String),
    Search,
    HomeRecent,
    HomeTop,
}

#[derive(Debug, Clone)]
pub enum Message {
    Ui(Ui),
    Net(Net),
    Done(Done),
}

#[derive(Debug, Clone)]
pub enum Ui {
    Navigate(Route),
    Back,
    Forward,
    SearchInput(String),
    SearchSubmit,
    SearchFilter(SearchFilter),
    LibraryFilter(LibraryFilter),
    /// Start playback from a list; `None` = from the top (random with shuffle).
    PlayFrom(TrackSource, Option<usize>),
    TogglePlay,
    Next,
    Previous,
    ToggleShuffle,
    CycleRepeat,
    SeekPreview(f32),
    SeekCommit,
    Volume(f32),
    VolumeCommit,
    ToggleMute,
    ToggleQueue,
    PlayUpcoming(usize),
    PlayQueued(usize),
    Enqueue(Track),
    ToggleLike(Track),
    HoverRow(Option<String>),
    SelectRow(String),
    Login,
    CancelLogin,
    OpenLoginUrl,
    CopyLoginUrl,
    Logout,
    Scrolled(f32),
    DismissToast,
    Tick(Instant),
}

#[derive(Debug, Clone)]
pub enum Net {
    Playback(PlaybackEvent),
    Media(MediaEvent),
}

/// A signed-in session ready for use.
#[derive(Debug, Clone)]
pub struct SessionReady {
    pub source: DataSource,
    pub user: Option<User>,
}

#[derive(Debug, Clone)]
pub enum Done {
    /// Keyring restore at boot: `None` = no stored session.
    Restored(Result<Option<SessionReady>>),
    LoggedIn(Result<SessionReady>),
    Profile(Result<User>),
    Audio(Result<SpotifySession>),
    Library(Result<Library>),
    Home(Result<HomeFeed>),
    Playlist(String, Result<(Playlist, Page<Track>)>),
    PlaylistMore(String, Result<Page<Track>>),
    Liked(Result<Page<Track>>),
    Album(String, Result<Album>),
    Artist(String, Result<ArtistPage>),
    SearchDebounced(u64),
    Search(u64, Result<SearchResults>),
    Image(String, Result<DecodedImage>),
    LikeSaved(String, bool, Result<()>),
    LikedFlags(Vec<String>, Result<Vec<bool>>),
    ReconnectAudio,
    Screenshot(iced::window::Screenshot),
    ScreenshotSaved(Result<()>),
    Quit,
    Noop,
}

pub fn ui(u: Ui) -> Message {
    Message::Ui(u)
}
