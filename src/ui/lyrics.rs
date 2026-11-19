//! Right-hand lyrics panel (time-synced lines highlight as the song plays).

use iced::widget::{button, column, container, row, scrollable};
use iced::{Alignment, Length, Padding};

use super::icons::Icon;
use super::theme::{self, FONT_BOLD, M, pal};
use super::widgets::{El, bold, fill, icon, label, loading, secondary, vspace};
use crate::app::{App, Ui, ui};

pub fn view(app: &App) -> El<'_> {
    let p = pal();
    let mut col = column![
        row![
            bold("Lyrics", M.font_lg),
            fill(),
            button(icon(Icon::Close, 18.0, p.text_secondary))
                .padding(6)
                .style(theme::round_dark)
                .on_press(ui(Ui::ToggleLyrics)),
        ]
        .align_y(Alignment::Center),
        vspace(8.0),
    ]
    .spacing(10);
    let current_id = app.now_track().map(|t| t.id.as_str());
    match (&app.lyrics, current_id) {
        (_, None) => col = col.push(secondary("Nothing is playing.", M.font_md)),
        (Some((id, Some(Some(lyrics)))), Some(cur)) if id == cur => {
            let pos = app.now_playback().position(app.now);
            let active = lyrics.current_line(pos);
            for (i, line) in lyrics.lines.iter().enumerate() {
                let color = match active {
                    Some(a) if i == a => p.text_primary,
                    Some(a) if i < a => p.text_muted,
                    _ => p.text_secondary,
                };
                let text = if line.text.trim().is_empty() {
                    "♪"
                } else {
                    line.text.as_str()
                };
                col = col.push(label(text, M.font_lg, color, FONT_BOLD));
            }
            if let Some(provider) = &lyrics.provider {
                col = col.push(vspace(8.0)).push(secondary(
                    format!("Lyrics provided by {provider}"),
                    M.font_sm,
                ));
            }
        }
        (Some((id, Some(None))), Some(cur)) if id == cur => {
            col = col.push(secondary("No lyrics for this song.", M.font_md));
        }
        _ => col = col.push(loading()),
    }
    container(
        scrollable(col.padding(Padding::from([12, 16])))
            .style(theme::thin_scroll)
            .height(Length::Fill),
    )
    .width(M.queue_width)
    .height(Length::Fill)
    .style(theme::panel)
    .into()
}
