//! Right-hand queue panel: now playing, next in queue, next from context.

use iced::widget::{button, column, container, row, scrollable};
use iced::{Alignment, Length, Padding};

use super::icons::Icon;
use super::theme::{self, FONT, M, pal};
use super::widgets::{El, art, bold, clipped, fill, icon, label, secondary, vspace};
use crate::app::{App, Message, Ui, ui};
use crate::net::model::Track;

fn entry<'a>(app: &App, t: &'a Track, current: bool, msg: Option<Message>) -> El<'a> {
    let color = if current {
        pal().accent
    } else {
        pal().text_primary
    };
    let r = row![
        art(app, t.images(), M.art_sidebar, false, Icon::Music),
        column![
            clipped(label(t.name.as_str(), M.font_md, color, FONT)),
            clipped(secondary(t.artist_line(), M.font_sm)),
        ]
        .spacing(2)
        .width(Length::Fill),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    button(r)
        .padding(8)
        .width(Length::Fill)
        .style(theme::list_row(false))
        .on_press_maybe(msg)
        .into()
}

pub fn view(app: &App) -> El<'_> {
    let q = &app.queue;
    let mut col = column![
        row![
            bold("Queue", M.font_lg),
            fill(),
            button(icon(Icon::Close, 18.0, pal().text_secondary))
                .padding(6)
                .style(theme::round_dark)
                .on_press(ui(Ui::ToggleQueue)),
        ]
        .align_y(Alignment::Center),
        vspace(8.0),
    ];
    match &q.current {
        Some(t) => {
            col = col
                .push(bold("Now playing", M.font_md))
                .push(entry(app, t, true, None));
        }
        None => col = col.push(secondary("Nothing is playing.", M.font_md)),
    }
    if !q.user_queue.is_empty() {
        col = col
            .push(vspace(16.0))
            .push(bold("Next in queue", M.font_md));
        for (k, t) in q.user_queue.iter().enumerate().take(50) {
            col = col.push(entry(app, t, false, Some(ui(Ui::PlayQueued(k)))));
        }
    }
    let upcoming = q.upcoming(50);
    if !upcoming.is_empty() {
        let from = q
            .context
            .as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_default();
        col = col
            .push(vspace(16.0))
            .push(bold(format!("Next from: {from}"), M.font_md));
        for (k, t) in upcoming.into_iter().enumerate() {
            col = col.push(entry(app, t, false, Some(ui(Ui::PlayUpcoming(k)))));
        }
    }
    container(
        scrollable(container(col.spacing(4)).padding(Padding::from([16, 12])))
            .style(theme::thin_scroll),
    )
    .width(M.queue_width)
    .height(Length::Fill)
    .style(theme::panel)
    .into()
}
