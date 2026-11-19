//! Search: browse tiles when empty, results grouped by type otherwise.

use iced::widget::{button, column, container, row};
use iced::{Alignment, Color, Length, Padding};

use super::icons::Icon;
use super::theme::{self, FONT, FONT_BOLD, M, pal};
use super::widgets::{
    El, TableOpts, art, bold, card, card_row, error_box, label, loading, secondary, section,
    track_table, vspace, wrap_grid,
};
use crate::app::menu::MenuTarget;
use crate::app::state::{Route, SearchFilter};
use crate::app::{App, TrackSource, Ui, ui};

const GENRES: &[(&str, u32)] = &[
    ("Pop", 0x8d67ab),
    ("Hip-Hop", 0xba5d07),
    ("Rock", 0xe91429),
    ("Indie", 0x608108),
    ("Electronic", 0x0d73ec),
    ("Jazz", 0x8c1932),
    ("Classical", 0x7d4b32),
    ("Chill", 0x477d95),
    ("Focus", 0x503750),
    ("Workout", 0x777777),
    ("Mood", 0xe1118c),
    ("Party", 0x537aa1),
];

fn tile<'a>(name: &'a str, color: u32) -> El<'a> {
    let bg = Color::from_rgb8((color >> 16) as u8, (color >> 8) as u8, color as u8);
    button(
        container(label(name, M.font_xl, Color::WHITE, FONT_BOLD))
            .padding(16)
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(196)
    .height(110)
    .padding(0)
    .style(move |_, status| iced::widget::button::Style {
        background: Some(
            if status == iced::widget::button::Status::Hovered {
                theme::mix(bg, Color::WHITE, 0.08)
            } else {
                bg
            }
            .into(),
        ),
        text_color: Color::WHITE,
        border: iced::Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .on_press(ui(Ui::SearchInput(name.to_owned())))
    .into()
}

fn filter_chip<'a>(name: &'a str, f: SearchFilter, cur: SearchFilter) -> El<'a> {
    let active = f == cur;
    button(label(
        name,
        M.font_sm,
        if active {
            pal().bg_app
        } else {
            pal().text_primary
        },
        FONT,
    ))
    .padding(Padding::from([6, 12]))
    .style(theme::chip(active))
    .on_press(ui(Ui::SearchFilter(f)))
    .into()
}

pub fn view(app: &App) -> El<'_> {
    let s = &app.search;
    let body: El<'_> = if s.query.trim().is_empty() {
        let tiles = GENRES
            .iter()
            .map(|(n, c)| {
                container(tile(n, *c))
                    .padding(Padding::from([0, 12]).bottom(24))
                    .into()
            })
            .collect();
        column![
            bold("Browse all", M.font_xl),
            vspace(16.0),
            wrap_grid(tiles)
        ]
        .into()
    } else {
        match &s.results {
            None if s.loading => loading(),
            None => match &s.error {
                Some(e) => error_box(e.clone()),
                None => loading(),
            },
            Some(r) if r.is_empty() => container(
                column![
                    bold(format!("No results found for \"{}\"", r.query), M.font_xl),
                    secondary(
                        "Please make sure your words are spelled correctly, or use fewer keywords.",
                        M.font_md
                    ),
                ]
                .spacing(8)
                .align_x(Alignment::Center),
            )
            .center_x(Length::Fill)
            .padding(64)
            .into(),
            Some(r) => {
                let f = s.filter;
                let chips = row![
                    filter_chip("All", SearchFilter::All, f),
                    filter_chip("Songs", SearchFilter::Songs, f),
                    filter_chip("Artists", SearchFilter::Artists, f),
                    filter_chip("Albums", SearchFilter::Albums, f),
                    filter_chip("Playlists", SearchFilter::Playlists, f),
                ]
                .spacing(8);
                let artists = || -> Vec<El<'_>> {
                    r.artists
                        .iter()
                        .map(|a| {
                            card(
                                app,
                                &a.images,
                                &a.name,
                                "Artist".into(),
                                true,
                                Icon::User,
                                ui(Ui::Navigate(Route::Artist(a.id.clone()))),
                                Some(MenuTarget::artist(a)),
                            )
                        })
                        .collect()
                };
                let albums = || -> Vec<El<'_>> {
                    r.albums
                        .iter()
                        .map(|a| {
                            card(
                                app,
                                &a.images,
                                &a.name,
                                format!("{} • {}", a.year(), a.artist_line()),
                                false,
                                Icon::Disc,
                                ui(Ui::Navigate(Route::Album(a.id.clone()))),
                                Some(MenuTarget::album(a)),
                            )
                        })
                        .collect()
                };
                let playlists = || -> Vec<El<'_>> {
                    r.playlists
                        .iter()
                        .map(|p| {
                            card(
                                app,
                                &p.images,
                                &p.name,
                                format!("By {}", p.owner),
                                false,
                                Icon::Music,
                                ui(Ui::Navigate(Route::Playlist(p.id.clone()))),
                                Some(app.playlist_target(p)),
                            )
                        })
                        .collect()
                };
                let opts = TableOpts {
                    album: false,
                    added: false,
                    art: true,
                    header: false,
                };
                let mut col = column![chips, vspace(20.0)];
                match f {
                    SearchFilter::All => {
                        let top: Option<El<'_>> = if let Some(a) = r.artists.first() {
                            Some(top_result(
                                app,
                                art(app, &a.images, 92.0, true, Icon::User),
                                &a.name,
                                "Artist".into(),
                                ui(Ui::Navigate(Route::Artist(a.id.clone()))),
                                Some(MenuTarget::artist(a)),
                            ))
                        } else {
                            r.tracks.first().map(|t| {
                                top_result(
                                    app,
                                    art(app, t.images(), 92.0, false, Icon::Music),
                                    &t.name,
                                    format!("Song • {}", t.artist_line()),
                                    ui(Ui::PlayFrom(TrackSource::Search, Some(0))),
                                    Some(MenuTarget::track(t, None)),
                                )
                            })
                        };
                        let songs = track_table(
                            app,
                            &r.tracks[..r.tracks.len().min(4)],
                            TrackSource::Search,
                            opts,
                        );
                        let mut top_row = row![].spacing(24);
                        if let Some(t) = top {
                            top_row = top_row.push(
                                container(section("Top result", t)).width(Length::FillPortion(2)),
                            );
                        }
                        top_row = top_row
                            .push(container(section("Songs", songs)).width(Length::FillPortion(3)));
                        col = col.push(top_row);
                        if !r.artists.is_empty() {
                            col = col
                                .push(vspace(24.0))
                                .push(section("Artists", card_row(artists())));
                        }
                        if !r.albums.is_empty() {
                            col = col
                                .push(vspace(24.0))
                                .push(section("Albums", card_row(albums())));
                        }
                        if !r.playlists.is_empty() {
                            col = col
                                .push(vspace(24.0))
                                .push(section("Playlists", card_row(playlists())));
                        }
                    }
                    SearchFilter::Songs => {
                        col = col.push(track_table(
                            app,
                            &r.tracks,
                            TrackSource::Search,
                            TableOpts {
                                album: true,
                                header: true,
                                ..opts
                            },
                        ));
                    }
                    SearchFilter::Artists => col = col.push(wrap_grid(artists())),
                    SearchFilter::Albums => col = col.push(wrap_grid(albums())),
                    SearchFilter::Playlists => col = col.push(wrap_grid(playlists())),
                }
                col.into()
            }
        }
    };
    container(body).padding(24).width(Length::Fill).into()
}

fn top_result<'a>(
    _app: &App,
    art_el: El<'a>,
    title: &'a str,
    subtitle: String,
    msg: crate::app::Message,
    target: Option<MenuTarget>,
) -> El<'a> {
    let b: El<'a> = button(
        column![
            art_el,
            vspace(12.0),
            bold(title, 32.0),
            secondary(subtitle, M.font_sm)
        ]
        .spacing(4),
    )
    .padding(20)
    .width(Length::Fill)
    .height(232)
    .style(|_, status| iced::widget::button::Style {
        background: Some(
            if status == iced::widget::button::Status::Hovered {
                pal().bg_selected
            } else {
                pal().bg_hover
            }
            .into(),
        ),
        text_color: pal().text_primary,
        border: iced::Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .on_press(msg)
    .into();
    match target {
        Some(t) => super::menu::right_click(b, t),
        None => b,
    }
}
