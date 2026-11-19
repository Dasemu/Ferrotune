//! Login screen: one "Log in" button that opens accounts.spotify.com in the
//! browser (like the official desktop app), and the waiting state.

use iced::widget::{button, column, container, row, text};
use iced::{Alignment, Color, Length, Padding};

use super::icons::Icon;
use super::theme::{self, FONT, FONT_BOLD, M, pal};
use super::widgets::{El, icon, label, secondary, vspace};
use crate::app::{App, Ui, ui};
use crate::auth::AuthState;

fn pill<'a>(t: &'a str, primary: bool, msg: crate::app::Message) -> El<'a> {
    let color = if primary {
        pal().accent_text
    } else {
        pal().text_primary
    };
    button(container(label(t, M.font_lg, color, FONT_BOLD)).center_x(Length::Fill))
        .padding(Padding::from([14, 32]))
        .width(Length::Fill)
        .style(if primary {
            theme::pill_primary
        } else {
            theme::pill_outline
        })
        .on_press(msg)
        .into()
}

fn para<'a>(s: impl text::IntoFragment<'a>) -> El<'a> {
    text(s)
        .size(M.font_md)
        .color(pal().text_secondary)
        .font(FONT)
        .align_x(Alignment::Center)
        .width(Length::Fill)
        .into()
}

pub fn view(app: &App) -> El<'_> {
    let logo = container(icon(Icon::Music, 30.0, Color::BLACK))
        .width(56)
        .height(56)
        .center_x(56)
        .center_y(56)
        .style(|_: &iced::Theme| container::Style {
            background: Some(pal().accent.into()),
            border: iced::Border {
                radius: 28.0.into(),
                ..Default::default()
            },
            ..Default::default()
        });
    let mut col = column![
        logo,
        vspace(16.0),
        label("Log in to Ferrotune", 32.0, Color::WHITE, FONT_BOLD),
        para("An unofficial, lightweight Spotify client. Spotify Premium is required."),
        vspace(24.0),
    ]
    .spacing(8)
    .align_x(Alignment::Center)
    .width(380);

    match &app.auth {
        AuthState::Restoring | AuthState::Connecting => {
            col = col.push(
                row![
                    icon(Icon::Loader, 18.0, pal().text_secondary),
                    secondary("Connecting…", M.font_md)
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        AuthState::SignedOut { error } => {
            col = col.push(pill("Log in", true, ui(Ui::Login)));
            col = col.push(vspace(8.0));
            col = col.push(para(
                "Your browser opens accounts.spotify.com. Approve the login there and come back: there is nothing to set up.",
            ));
            if let Some(e) = error {
                col = col.push(vspace(8.0)).push(
                    row![
                        icon(Icon::Alert, 16.0, pal().danger),
                        label(e.as_str(), M.font_md, pal().danger, FONT)
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                );
            }
        }
        AuthState::WaitingForBrowser { .. } => {
            col = col
                .push(para(
                    "Finish logging in in your browser. This window updates automatically.",
                ))
                .push(vspace(16.0))
                .push(pill("Open browser again", true, ui(Ui::OpenLoginUrl)))
                .push(pill("Copy login link", false, ui(Ui::CopyLoginUrl)))
                .push(pill("Cancel", false, ui(Ui::CancelLogin)));
        }
        AuthState::SignedIn { .. } => {}
    }

    let card = container(col)
        .padding(Padding::from([48, 64]))
        .style(theme::login_card);
    container(column![
        container(card).center(Length::Fill),
        container(secondary(
            "Unofficial client, not affiliated with Spotify AB. Third-party clients may break Spotify's terms and put your account at risk.",
            M.font_xs + 1.0
        ))
        .center_x(Length::Fill)
        .padding(16),
    ])
    .width(Length::Fill)
    .height(Length::Fill)
    .style(theme::login_bg)
    .into()
}
