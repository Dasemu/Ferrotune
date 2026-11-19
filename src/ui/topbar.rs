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
    let images: Vec<crate::net::model::Image> = app
        .user
        .as_ref()
        .and_then(|u| u.image.clone())
        .map(|url| crate::net::model::Image {
            url,
            width: Some(64),
            height: Some(64),
        })
        .into_iter()
        .collect();
    let face: El<'_> = if app.image(&images, 64).is_some() {
        super::widgets::art(app, &images, 32.0, true, Icon::User)
    } else {
        container(label(initial, M.font_md, iced::Color::BLACK, FONT_BOLD))
            .width(32)
            .height(32)
            .center_x(32)
            .center_y(32)
            .style(theme::avatar)
            .into()
    };
    let avatar = button(face)
        .padding(4)
        .style(theme::round_dark)
        .on_press(ui(Ui::ToggleUserMenu));

    let bar = row![
        nav,
        fill(),
        row![home, search_box].spacing(8).align_y(Alignment::Center),
        fill(),
        row![avatar].spacing(4).align_y(Alignment::Center),
    ]
    .align_y(Alignment::Center)
    .padding(Padding::from([0, 8]));
    container(bar)
        .height(M.topbar_height)
        .width(Length::Fill)
        .align_y(Alignment::Center)
        .into()
}

/// Account menu under the avatar: Profile, Settings, Log out.
pub fn user_menu(app: &App) -> El<'_> {
    use iced::widget::{Space, column, mouse_area, stack};
    let item = |text: &'static str, msg: Ui| -> El<'_> {
        button(
            container(label(text, M.font_md, pal().text_primary, theme::FONT))
                .center_y(Length::Fill),
        )
        .padding(Padding::from([0, 12]))
        .height(theme::MENU_ROW)
        .width(Length::Fill)
        .style(theme::menu_item)
        .on_press(ui(msg))
        .into()
    };
    let me = app.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
    let mut col = column![].spacing(0);
    if !me.is_empty() {
        col = col.push(item("Profile", Ui::Navigate(Route::Profile(me))));
    }
    col = col
        .push(item("Settings", Ui::Navigate(Route::Settings)))
        .push(
            container(Space::new().height(1))
                .width(Length::Fill)
                .style(theme::separator),
        )
        .push(item("Log out", Ui::Logout));
    let panel = container(col.padding(4))
        .width(196)
        .style(theme::menu_panel);
    let backdrop = mouse_area(
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .on_press(ui(Ui::ToggleUserMenu));
    let placed = container(panel)
        .width(Length::Fill)
        .align_x(Alignment::End)
        .padding(Padding {
            top: M.topbar_height - 4.0,
            right: 16.0,
            ..Padding::ZERO
        });
    stack![backdrop, placed].into()
}
