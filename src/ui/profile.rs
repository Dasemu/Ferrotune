//! Profile page: round avatar header, followers / following and the
//! user's public playlists (`user-profile-view`).

use iced::Color;

use super::icons::Icon;
use super::theme::{self, M};
use super::widgets::{
    El, art, error_box, header, loading, playlist_card, secondary, section, wrap_grid,
};
use crate::app::state::{Route, format_count};
use crate::app::{App, Ui, ui};
use crate::net::model::Image;

pub fn view<'a>(app: &'a App, user: &'a str) -> El<'a> {
    let route = Route::Profile(user.to_owned());
    let Some(p) = app.profiles.get(user) else {
        return match app.errors.get(&route) {
            Some(e) => error_box(e.clone()),
            None => loading(),
        };
    };
    let images: Vec<Image> = p
        .image
        .iter()
        .map(|u| Image {
            url: u.clone(),
            width: Some(300),
            height: Some(300),
        })
        .collect();
    let tint = app
        .art_color(&images)
        .map(theme::header_tint)
        .unwrap_or_else(|| Color::from_rgb8(0x53, 0x53, 0x53));
    let mut parts = Vec::new();
    if let Some(n) = p.total_playlists.filter(|n| *n > 0) {
        parts.push(format!("{n} Public Playlists"));
    }
    if let Some(n) = p.followers {
        parts.push(format!("{} Followers", format_count(n)));
    }
    if let Some(n) = p.following {
        parts.push(format!("{} Following", format_count(n)));
    }
    let meta: El<'_> = secondary(parts.join(" • "), M.font_sm).into();
    let cards: Vec<El<'_>> = p
        .playlists
        .iter()
        .map(|pl| {
            playlist_card(
                app,
                pl,
                format!("By {}", app.owner_label(&pl.owner)),
                ui(Ui::Navigate(Route::Playlist(pl.id.clone()))),
                Some(app.playlist_target(pl)),
            )
        })
        .collect();
    let mut col = iced::widget::column![header(
        tint,
        art(app, &images, M.art_header, true, Icon::User),
        "Profile",
        p.name.clone(),
        None,
        meta,
    )];
    if !cards.is_empty() {
        col = col.push(
            iced::widget::container(section("Public Playlists", wrap_grid(cards)))
                .padding(iced::Padding::from([16, 12])),
        );
    }
    col.into()
}
