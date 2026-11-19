//! Full-width "Your Library" grid.

use iced::widget::{button, column, container, row};
use iced::{Length, Padding};

use super::icons::Icon;
use super::theme::{self, FONT, M, pal};
use super::widgets::{El, bold, card, label, vspace, wrap_grid};
use crate::app::state::{LibraryFilter, Route};
use crate::app::{App, Ui, ui};

pub fn view(app: &App) -> El<'_> {
    let f = app.library_filter;
    let mut chips = row![].spacing(8);
    for (name, kind) in [
        ("Playlists", LibraryFilter::Playlists),
        ("Albums", LibraryFilter::Albums),
        ("Artists", LibraryFilter::Artists),
    ] {
        let active = f == kind;
        chips = chips.push(
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
            .on_press(ui(Ui::LibraryFilter(kind))),
        );
    }
    let lib = &app.library;
    let mut cards = Vec::new();
    if matches!(f, LibraryFilter::All | LibraryFilter::Playlists) {
        for p in &lib.playlists {
            cards.push(card(
                app,
                &p.images,
                &p.name,
                format!("Playlist • {}", p.owner),
                false,
                Icon::Music,
                ui(Ui::Navigate(Route::Playlist(p.id.clone()))),
            ));
        }
    }
    if matches!(f, LibraryFilter::All | LibraryFilter::Albums) {
        for a in &lib.albums {
            cards.push(card(
                app,
                &a.images,
                &a.name,
                a.artist_line(),
                false,
                Icon::Disc,
                ui(Ui::Navigate(Route::Album(a.id.clone()))),
            ));
        }
    }
    if matches!(f, LibraryFilter::All | LibraryFilter::Artists) {
        for a in &lib.artists {
            cards.push(card(
                app,
                &a.images,
                &a.name,
                "Artist".into(),
                true,
                Icon::User,
                ui(Ui::Navigate(Route::Artist(a.id.clone()))),
            ));
        }
    }
    container(column![
        bold("Your Library", M.font_xl),
        vspace(12.0),
        chips,
        vspace(16.0),
        wrap_grid(cards)
    ])
    .padding(24)
    .width(Length::Fill)
    .into()
}
