//! Home: greeting, shortcut tiles and card rows.

use std::collections::HashSet;

use iced::widget::{button, column, container, row};
use iced::{Alignment, Length, Padding};

use super::icons::Icon;
use super::theme::{self, FONT, M, pal};
use super::widgets::{
    El, art, bold, card, card_row, clipped, label, liked_art, loading, section, semibold, vspace,
};
use crate::app::state::Route;
use crate::app::{App, Message, TrackSource, Ui, ui};
use crate::net::model::Image;

fn greeting() -> &'static str {
    let secs = crate::auth::oauth::now_secs();
    // Local time is not available without a tz crate; UTC is close enough
    // for a greeting.
    match (secs / 3600) % 24 {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        _ => "Good evening",
    }
}

fn shortcut<'a>(art_el: El<'a>, title: &'a str, msg: Message) -> El<'a> {
    button(
        row![art_el, clipped(semibold(title, M.font_md))]
            .spacing(12)
            .align_y(Alignment::Center),
    )
    .padding(0)
    .width(Length::Fill)
    .height(56)
    .style(theme::shortcut)
    .on_press(msg)
    .into()
}

fn shortcut_art<'a>(app: &App, images: &[Image]) -> El<'a> {
    art(app, images, 56.0, false, Icon::Music)
}

pub fn view(app: &App) -> El<'_> {
    let p = pal();
    let mut tiles: Vec<El<'_>> = vec![shortcut(
        liked_art(56.0),
        "Liked Songs",
        ui(Ui::Navigate(Route::Liked)),
    )];
    for pl in app.library.playlists.iter().take(5) {
        tiles.push(shortcut(
            shortcut_art(app, &pl.images),
            &pl.name,
            ui(Ui::Navigate(Route::Playlist(pl.id.clone()))),
        ));
    }
    for al in app.library.albums.iter().take(8 - tiles.len()) {
        tiles.push(shortcut(
            shortcut_art(app, &al.images),
            &al.name,
            ui(Ui::Navigate(Route::Album(al.id.clone()))),
        ));
    }
    let mut grid = column![].spacing(8);
    let mut it = tiles.into_iter();
    loop {
        let (Some(a), b) = (it.next(), it.next()) else {
            break;
        };
        let c = it.next();
        let d = it.next();
        let mut r = row![a].spacing(8);
        for x in [b, c, d].into_iter().flatten() {
            r = r.push(x);
        }
        grid = grid.push(r);
    }

    let chips = row![
        button(label("All", M.font_sm, p.bg_app, FONT))
            .padding(Padding::from([6, 12]))
            .style(theme::chip(true)),
        button(label("Music", M.font_sm, p.text_primary, FONT))
            .padding(Padding::from([6, 12]))
            .style(theme::chip(false)),
    ]
    .spacing(8);

    let mut col = column![
        chips,
        vspace(16.0),
        bold(greeting(), M.font_xl),
        vspace(12.0),
        grid
    ]
    .spacing(0);

    match &app.home {
        None if app.loading.contains(&Route::Home) => col = col.push(loading()),
        None => {}
        Some(h) => {
            // "Jump back in": unique albums of recently played tracks.
            let mut seen = HashSet::new();
            let recent: Vec<El<'_>> = h
                .recent_tracks
                .iter()
                .filter_map(|t| t.album.as_ref())
                .filter(|a| seen.insert(a.id.clone()))
                .take(6)
                .map(|a| {
                    card(
                        app,
                        &a.images,
                        &a.name,
                        "Album".into(),
                        false,
                        Icon::Disc,
                        ui(Ui::Navigate(Route::Album(a.id.clone()))),
                    )
                })
                .collect();
            if !recent.is_empty() {
                col = col
                    .push(vspace(32.0))
                    .push(section("Jump back in", card_row(recent)));
            }
            let artists: Vec<El<'_>> = h
                .top_artists
                .iter()
                .take(6)
                .map(|a| {
                    card(
                        app,
                        &a.images,
                        &a.name,
                        "Artist".into(),
                        true,
                        Icon::User,
                        ui(Ui::Navigate(Route::Artist(a.id.clone()))),
                    )
                })
                .collect();
            if !artists.is_empty() {
                col = col
                    .push(vspace(24.0))
                    .push(section("Your top artists", card_row(artists)));
            }
            let tops: Vec<El<'_>> = h
                .top_tracks
                .iter()
                .enumerate()
                .take(6)
                .map(|(i, t)| {
                    card(
                        app,
                        t.images(),
                        &t.name,
                        t.artist_line(),
                        false,
                        Icon::Music,
                        ui(Ui::PlayFrom(TrackSource::HomeTop, Some(i))),
                    )
                })
                .collect();
            if !tops.is_empty() {
                col = col
                    .push(vspace(24.0))
                    .push(section("On repeat", card_row(tops)));
            }
        }
    }
    let playlists: Vec<El<'_>> = app
        .library
        .playlists
        .iter()
        .take(6)
        .map(|pl| {
            card(
                app,
                &pl.images,
                &pl.name,
                pl.description.clone(),
                false,
                Icon::Music,
                ui(Ui::Navigate(Route::Playlist(pl.id.clone()))),
            )
        })
        .collect();
    if !playlists.is_empty() {
        col = col
            .push(vspace(24.0))
            .push(section("Your playlists", card_row(playlists)));
    }
    container(col)
        .padding(Padding {
            top: 16.0,
            right: 24.0,
            bottom: 32.0,
            left: 24.0,
        })
        .width(Length::Fill)
        .style(theme::gradient_header(
            theme::mix(
                pal().bg_panel,
                iced::Color::from_rgb8(0x3a, 0x2a, 0x6a),
                0.35,
            ),
            pal().bg_panel,
        ))
        .into()
}
