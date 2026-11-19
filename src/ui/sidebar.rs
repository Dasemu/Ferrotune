//! Left "Your Library" panel with filter chips.

use iced::widget::{button, column, container, row, scrollable};
use iced::{Alignment, Length, Padding};

use super::icons::Icon;
use super::theme::{self, FONT, M, pal};
use super::widgets::{El, art, bold, clipped, fill, hover_icon, icon, label, liked_art, secondary};
use crate::app::menu::MenuTarget;
use crate::app::state::{LibraryFilter, Route};
use crate::app::{App, Ui, ui};
use crate::net::model::Image;

fn chip<'a>(name: &'a str, f: LibraryFilter, current: LibraryFilter) -> El<'a> {
    let active = f == current;
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
    .on_press(ui(Ui::LibraryFilter(f)))
    .into()
}

fn item<'a>(
    app: &'a App,
    art_el: El<'a>,
    title: &'a str,
    subtitle: String,
    route: Route,
    target: Option<MenuTarget>,
) -> El<'a> {
    let selected = *app.route() == route;
    let playing_here = app
        .queue
        .context
        .as_ref()
        .is_some_and(|c| c.route.as_ref() == Some(&route));
    let title_color = if playing_here {
        pal().accent
    } else {
        pal().text_primary
    };
    let mut r = row![
        art_el,
        column![
            clipped(label(title, M.font_lg, title_color, FONT)),
            clipped(secondary(subtitle, M.font_sm)),
        ]
        .spacing(2)
        .width(Length::Fill),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    if playing_here && app.playback.is_playing() {
        r = r.push(icon(Icon::Volume, 16.0, pal().accent));
    }
    let b: El<'a> = button(r)
        .padding(8)
        .width(Length::Fill)
        .style(theme::list_row(selected))
        .on_press(ui(Ui::Navigate(route)))
        .into();
    match target {
        Some(t) => super::menu::right_click(b, t),
        None => b,
    }
}

fn sidebar_art<'a>(app: &App, images: &[Image], round: bool, fallback: Icon) -> El<'a> {
    art(app, images, M.art_sidebar, round, fallback)
}

pub fn view(app: &App) -> El<'_> {
    let f = app.library_filter;
    let header = row![
        button(
            row![
                hover_icon(
                    Icon::Library,
                    24.0,
                    pal().text_secondary,
                    pal().text_primary
                ),
                bold("Your Library", M.font_lg)
            ]
            .spacing(12)
            .align_y(Alignment::Center)
        )
        .padding(4)
        .style(theme::icon_button)
        .on_press(ui(Ui::Navigate(Route::Library))),
        fill(),
        button(hover_icon(
            Icon::Plus,
            18.0,
            pal().text_secondary,
            pal().text_primary
        ))
        .padding(8)
        .style(theme::round_dark)
        .on_press(ui(Ui::NewPlaylist)),
    ]
    .align_y(Alignment::Center);

    let mut chips = row![].spacing(8).align_y(Alignment::Center);
    if f != LibraryFilter::All {
        chips = chips.push(
            button(icon(Icon::Close, 14.0, pal().text_primary))
                .padding(7)
                .style(theme::chip(false))
                .on_press(ui(Ui::LibraryFilter(f))),
        );
    }
    for (name, kind) in [
        ("Playlists", LibraryFilter::Playlists),
        ("Albums", LibraryFilter::Albums),
        ("Artists", LibraryFilter::Artists),
    ] {
        if f == LibraryFilter::All || f == kind {
            chips = chips.push(chip(name, kind, f));
        }
    }

    let lib = &app.library;
    let mut list = column![].spacing(0);
    if matches!(f, LibraryFilter::All | LibraryFilter::Playlists) {
        list = list.push(item(
            app,
            liked_art(M.art_sidebar),
            "Liked Songs",
            format!("Playlist • {} songs", lib.liked_total),
            Route::Liked,
            None,
        ));
        for p in &lib.playlists {
            list = list.push(item(
                app,
                super::widgets::playlist_art(app, p, M.art_sidebar),
                if p.name.trim().is_empty() {
                    "Playlist"
                } else {
                    p.name.as_str()
                },
                format!("Playlist • {}", app.owner_label(&p.owner)),
                Route::Playlist(p.id.clone()),
                Some(app.playlist_target(p)),
            ));
        }
    }
    if matches!(f, LibraryFilter::All | LibraryFilter::Albums) {
        for a in &lib.albums {
            list = list.push(item(
                app,
                sidebar_art(app, &a.images, false, Icon::Disc),
                &a.name,
                format!("Album • {}", a.artist_line()),
                Route::Album(a.id.clone()),
                Some(MenuTarget::album(a)),
            ));
        }
    }
    if matches!(f, LibraryFilter::All | LibraryFilter::Artists) {
        for a in &lib.artists {
            list = list.push(item(
                app,
                sidebar_art(app, &a.images, true, Icon::User),
                &a.name,
                "Artist".into(),
                Route::Artist(a.id.clone()),
                Some(MenuTarget::artist(a)),
            ));
        }
    }

    let body = column![
        container(header).padding(Padding {
            top: 12.0,
            right: 12.0,
            bottom: 4.0,
            left: 12.0
        }),
        container(chips).padding(Padding::from([8, 16])),
        scrollable(container(list).padding(Padding::from([0, 8])))
            .style(theme::thin_scroll)
            .height(Length::Fill),
    ]
    .spacing(4);
    container(body)
        .width(M.sidebar_width)
        .height(Length::Fill)
        .style(theme::panel)
        .into()
}
