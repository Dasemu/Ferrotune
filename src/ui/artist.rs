//! Artist page: hero header, popular tracks, discography.

use iced::widget::{column, container, row};
use iced::{Alignment, Color, Length, Padding};

use super::icons::Icon;
use super::theme::{self, M};
use super::widgets::{
    El, TableOpts, action_band, art, big_play, bold, card, card_row, error_box, header,
    icon_button, loading, secondary, section, track_table, vspace,
};
use crate::app::state::{Route, format_count};
use crate::app::{App, TrackSource, Ui, ui};

pub fn view<'a>(app: &'a App, id: &'a str) -> El<'a> {
    let route = Route::Artist(id.to_owned());
    let Some(page) = app.artists.get(id) else {
        return match app.errors.get(&route) {
            Some(e) => error_box(e.clone()),
            None => loading(),
        };
    };
    let a = &page.artist;
    let tint = app
        .art_color(&a.images)
        .map(theme::header_tint)
        .unwrap_or_else(|| Color::from_rgb8(0x53, 0x53, 0x53));
    let meta: El<'_> = secondary(
        a.followers
            .map(|f| format!("{} followers", format_count(f)))
            .unwrap_or_default(),
        M.font_md,
    )
    .into();
    let src = TrackSource::ArtistTop(id.to_owned());
    let albums: Vec<El<'_>> = page
        .albums
        .iter()
        .map(|al| {
            card(
                app,
                &al.images,
                &al.name,
                format!("{} • {}", al.year(), al.kind.label()),
                false,
                Icon::Disc,
                ui(Ui::Navigate(Route::Album(al.id.clone()))),
            )
        })
        .collect();
    let popular = track_table(
        app,
        &page.top_tracks[..page.top_tracks.len().min(10)],
        src.clone(),
        TableOpts {
            album: true,
            added: false,
            art: true,
            header: false,
        },
    );
    column![
        header(
            tint,
            art(app, &a.images, M.art_header, true, Icon::User),
            "Artist",
            a.name.clone(),
            None,
            meta
        ),
        action_band(
            tint,
            row![
                big_play(app.is_playing_source(&src), ui(Ui::PlayFrom(src, None))),
                icon_button(
                    Icon::Shuffle,
                    28.0,
                    Some(ui(Ui::ToggleShuffle)),
                    app.queue.shuffle
                ),
            ]
            .spacing(16)
            .align_y(Alignment::Center)
            .into()
        ),
        container(column![
            section("Popular", popular),
            vspace(32.0),
            if albums.is_empty() {
                bold("", 1.0).into()
            } else {
                section("Discography", card_row(albums))
            },
        ])
        .padding(Padding {
            top: 0.0,
            right: 24.0,
            bottom: 32.0,
            left: 24.0,
        })
        .width(Length::Fill),
    ]
    .into()
}
