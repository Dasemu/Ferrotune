//! Top bar: back/forward, home, search pill, account.

use iced::widget::{button, container, row, text_input};

use iced::{Alignment, Length, Padding};

use super::icons::Icon;
use super::theme::{self, FONT_BOLD, M, pal};
use super::widgets::{El, fill, icon, label};
use crate::app::state::Route;
use crate::app::{App, Ui, ui};

pub const SEARCH_ID: &str = "search-input";

fn round<'a>(i: Icon, size: f32, pad: u16, msg: Option<crate::app::Message>) -> El<'a> {
    let color = if msg.is_some() {
        pal().text_primary
    } else {
        pal().text_muted
    };
    button(icon(i, size, color))
        .padding(pad)
        .style(theme::round_dark)
        .on_press_maybe(msg)
        .into()
}

pub fn view(app: &App) -> El<'_> {
    let nav = row![
        round(
            Icon::ChevronLeft,
            18.0,
            7,
            app.nav.can_back().then(|| ui(Ui::Back))
        ),
        round(
            Icon::ChevronRight,
            18.0,
            7,
            app.nav.can_forward().then(|| ui(Ui::Forward))
        ),
    ]
    .spacing(8);

    let home_active = *app.route() == Route::Home;
    let home = button(icon(
        Icon::Home,
        22.0,
        if home_active {
            pal().text_primary
        } else {
            pal().text_secondary
        },
    ))
    .padding(12)
    .style(theme::round_dark)
    .on_press(ui(Ui::Navigate(Route::Home)));

    let search = text_input("What do you want to play?", &app.search.query)
        .id(SEARCH_ID)
        .on_input(|q| ui(Ui::SearchInput(q)))
        .on_submit(ui(Ui::SearchSubmit))
        .padding(Padding {
            top: 12.0,
            right: 16.0,
            bottom: 12.0,
            left: 48.0,
        })
        .size(M.font_lg)
        .width(420)
        .style(theme::search_input);

    let search_box = iced::widget::stack![
        search,
        container(icon(Icon::Search, 22.0, pal().text_secondary))
            .padding(Padding {
                left: 14.0,
                ..Padding::ZERO
            })
            .height(Length::Fill)
            .align_y(Alignment::Center),
    ];

    let initial: String = app
        .user
        .as_ref()
        .and_then(|u| u.display_name.chars().next())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".into());
    let avatar = container(label(initial, M.font_md, iced::Color::BLACK, FONT_BOLD))
        .width(40)
        .height(40)
        .center_x(40)
        .center_y(40)
        .style(theme::avatar);
    let logout = button(icon(Icon::LogOut, 18.0, pal().text_secondary))
        .padding(10)
        .style(theme::icon_button)
        .on_press(ui(Ui::Logout));

    let bar = row![
        nav,
        fill(),
        row![home, search_box].spacing(8).align_y(Alignment::Center),
        fill(),
        row![logout, avatar].spacing(4).align_y(Alignment::Center),
    ]
    .align_y(Alignment::Center)
    .padding(Padding::from([0, 8]));
    container(bar)
        .height(M.topbar_height)
        .width(Length::Fill)
        .align_y(Alignment::Center)
        .into()
}
