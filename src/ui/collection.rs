//! Playlist, album and Liked Songs views: gradient header + track table.

use iced::widget::{column, container, row};
use iced::{Alignment, Color, Length, Padding};

use super::icons::Icon;
use super::menu::click_menu;
use super::theme::{self, M};
use super::widgets::{
    El, TableOpts, action_band, art, artist_links, big_play, error_box, header, hover_icon,
    icon_button, liked_art, loading, meta_line, secondary, semibold, track_table,
};
use crate::app::menu::MenuTarget;
use crate::app::state::{Route, format_total};
use crate::app::{App, TrackSource, Ui, ui};
use crate::net::model::Track;

fn total_ms(tracks: &[Track]) -> u64 {
    tracks.iter().map(|t| u64::from(t.duration_ms)).sum()
}

fn tint_for(app: &App, images: &[crate::net::model::Image]) -> Color {
    app.art_color(images)
        .map(theme::header_tint)
        .unwrap_or_else(|| Color::from_rgb8(0x53, 0x53, 0x53))
}

fn actions<'a>(app: &App, src: TrackSource, tint: Color, more: Option<MenuTarget>) -> El<'a> {
    let playing = app.is_playing_source(&src);
    action_band(
        tint,
        row![
            big_play(playing, ui(Ui::PlayFrom(src, None))),
            icon_button(
                Icon::Shuffle,
                28.0,
                Some(ui(Ui::ToggleShuffle)),
                app.queue.shuffle
            ),
            more_button(more),
        ]
        .spacing(16)
        .align_y(Alignment::Center)
        .into(),
    )
}

/// The "..." button: opens the context menu of the list on click.
fn more_button<'a>(target: Option<MenuTarget>) -> El<'a> {
    match target {
        Some(t) => click_menu(
            container(hover_icon(
                Icon::More,
                28.0,
                theme::pal().text_secondary,
                theme::pal().text_primary,
            ))
            .padding(8),
            t,
        ),
        None => icon_button(Icon::More, 28.0, None, false),
    }
}

fn body<'a>(table: El<'a>, footer: Option<El<'a>>) -> El<'a> {
    let mut c = column![table];
    if let Some(f) = footer {
        c = c.push(f);
    }
    container(c)
        .padding(Padding {
            top: 0.0,
            right: 24.0,
            bottom: 32.0,
            left: 24.0,
        })
        .width(Length::Fill)
        .into()
}

fn state<'a>(app: &App, route: &Route) -> El<'a> {
    match app.errors.get(route) {
        Some(e) => error_box(e.clone()),
        None => loading(),
    }
}

pub fn playlist<'a>(app: &'a App, id: &'a str) -> El<'a> {
    let route = Route::Playlist(id.to_owned());
    let Some((pl, list)) = app.playlists.get(id) else {
        return state(app, &route);
    };
    let tint = tint_for(app, &pl.images);
    let meta = meta_line(vec![
        iced::widget::button(semibold(app.owner_label(&pl.owner), M.font_sm))
            .padding(0)
            .style(theme::link(false))
            .on_press_maybe(crate::app::meta::looks_like_username(&pl.owner).then(|| {
                ui(Ui::Navigate(Route::Profile(
                    crate::app::playlists::owner_username(&pl.owner),
                )))
            }))
            .into(),
        secondary(
            format!(
                "{} songs, {}",
                pl.total_tracks.max(list.total),
                format_total(total_ms(&list.tracks))
            ),
            M.font_sm,
        )
        .into(),
    ]);
    let src = TrackSource::Playlist(id.to_owned());
    let footer = list.has_more().then(|| {
        container(secondary("Scroll to load more…", M.font_sm))
            .padding(16)
            .into()
    });
    column![
        header(
            tint,
            super::widgets::playlist_art(app, pl, M.art_header),
            "Playlist",
            pl.name.clone(),
            Some(pl.description.clone()),
            meta
        ),
        actions(app, src.clone(), tint, Some(app.playlist_target(pl))),
        body(
            track_table(
                app,
                &list.tracks,
                src,
                TableOpts {
                    album: true,
                    added: true,
                    art: true,
                    header: true
                }
            ),
            footer
        ),
    ]
    .into()
}

pub fn album<'a>(app: &'a App, id: &'a str) -> El<'a> {
    let route = Route::Album(id.to_owned());
    let Some(a) = app.albums.get(id) else {
        return state(app, &route);
    };
    let tint = tint_for(app, &a.images);
    let meta = meta_line(vec![
        container(artist_links(&a.artists, M.font_sm, false))
            .width(Length::Shrink)
            .into(),
        secondary(a.year(), M.font_sm).into(),
        secondary(
            format!(
                "{} songs, {}",
                a.tracks.len(),
                format_total(total_ms(&a.tracks))
            ),
            M.font_sm,
        )
        .into(),
    ]);
    let src = TrackSource::Album(id.to_owned());
    let footer = (!a.release_date.is_empty()).then(|| {
        container(secondary(
            super::widgets::format_date(&a.release_date),
            M.font_sm,
        ))
        .padding(Padding::from([24, 16]))
        .into()
    });
    column![
        header(
            tint,
            art(app, &a.images, M.art_header, false, Icon::Disc),
            a.kind.label(),
            a.name.clone(),
            None,
            meta
        ),
        actions(app, src.clone(), tint, Some(MenuTarget::album(a))),
        body(
            track_table(
                app,
                &a.tracks,
                src,
                TableOpts {
                    album: false,
                    added: false,
                    art: false,
                    header: true
                }
            ),
            footer
        ),
    ]
    .into()
}

pub fn liked(app: &App) -> El<'_> {
    let Some(list) = &app.liked else {
        return state(app, &Route::Liked);
    };
    let tint = Color::from_rgb8(0x50, 0x38, 0xa0);
    let user = app
        .user
        .as_ref()
        .map(|u| u.display_name.clone())
        .unwrap_or_default();
    let meta = meta_line(vec![
        semibold(user, M.font_sm).into(),
        secondary(format!("{} songs", list.total), M.font_sm).into(),
    ]);
    let footer = list.has_more().then(|| {
        container(secondary("Scroll to load more…", M.font_sm))
            .padding(16)
            .into()
    });
    column![
        header(
            tint,
            liked_art(M.art_header),
            "Playlist",
            "Liked Songs".into(),
            None,
            meta
        ),
        actions(app, TrackSource::Liked, tint, None),
        body(
            track_table(
                app,
                &list.tracks,
                TrackSource::Liked,
                TableOpts {
                    album: true,
                    added: true,
                    art: true,
                    header: true
                }
            ),
            footer
        ),
    ]
    .into()
}
