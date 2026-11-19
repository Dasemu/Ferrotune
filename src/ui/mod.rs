//! Views: pure functions `&App -> Element<Message>`.

pub mod artist;
pub mod collection;
pub mod home;
pub mod icons;
pub mod library;
pub mod login;
pub mod now_playing;
pub mod queue;
pub mod search;
pub mod sidebar;
pub mod theme;
pub mod topbar;
pub mod widgets;

use iced::widget::{column, container, row, scrollable, stack};
use iced::{Alignment, Element, Length, Padding};

use crate::app::state::{Connection, Route};
use crate::app::{App, Message, Ui, ui};
use theme::M;
use widgets::{El, icon, primary};

pub fn view(app: &App) -> Element<'_, Message> {
    app.mark_frame();
    if !app.auth.is_signed_in() {
        return login::view(app);
    }
    let mut panels = row![sidebar::view(app), main_panel(app)]
        .spacing(M.gap)
        .height(Length::Fill);
    if app.show_queue {
        panels = panels.push(queue::view(app));
    }
    let body = column![topbar::view(app), panels, now_playing::view(app)]
        .spacing(0)
        .padding(Padding {
            top: 0.0,
            right: M.gap,
            bottom: 0.0,
            left: M.gap,
        });
    let base = container(body)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::app_bg);
    match &app.toast {
        Some((msg, _)) => stack![base, toast(msg)].into(),
        None => base.into(),
    }
}

fn toast<'a>(msg: &'a str) -> El<'a> {
    let t = iced::widget::button(
        container(primary(msg, M.font_md))
            .padding(Padding::from([10, 16]))
            .style(theme::toast),
    )
    .padding(0)
    .style(|_, _| iced::widget::button::Style::default())
    .on_press(ui(Ui::DismissToast));
    container(t)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::End)
        .padding(Padding {
            bottom: M.player_height + 24.0,
            ..Padding::ZERO
        })
        .into()
}

fn banner(app: &App) -> Option<El<'_>> {
    if app.is_mock() {
        return None;
    }
    let msg = match app.connection {
        Connection::Connected => return None,
        Connection::Connecting | Connection::Disconnected => "Connecting to Spotify…".to_owned(),
        Connection::Reconnecting { attempt } => {
            format!(
                "Offline — showing cached data. Reconnecting (attempt {})…",
                attempt + 1
            )
        }
    };
    Some(
        container(
            row![
                icon(icons::Icon::Loader, 14.0, theme::pal().text_secondary),
                widgets::secondary(msg, M.font_sm)
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([6, 12]))
        .width(Length::Fill)
        .style(theme::banner)
        .into(),
    )
}

fn main_panel(app: &App) -> El<'_> {
    let content: El<'_> = match app.route() {
        Route::Home => home::view(app),
        Route::Search => search::view(app),
        Route::Library => library::view(app),
        Route::Liked => collection::liked(app),
        Route::Playlist(id) => collection::playlist(app, id),
        Route::Album(id) => collection::album(app, id),
        Route::Artist(id) => artist::view(app, id),
    };
    let scroll = scrollable(content)
        .on_scroll(|v| ui(Ui::Scrolled(v.relative_offset().y)))
        .style(theme::thin_scroll)
        .height(Length::Fill)
        .width(Length::Fill);
    let mut col = column![].spacing(0);
    if let Some(b) = banner(app) {
        col = col.push(container(b).padding(8));
    }
    col = col.push(scroll);
    container(col)
        .width(Length::Fill)
        .height(Length::Fill)
        .clip(true)
        .style(theme::panel)
        .into()
}
