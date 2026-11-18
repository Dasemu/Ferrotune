//! Bottom "now playing" bar: track info, transport, seek bar, volume.

use iced::widget::{button, column, container, row, slider};
use iced::{Alignment, Color, Length};

use super::icons::Icon;
use super::theme::{self, M, pal};
use super::widgets::{
    El, art, artist_links, clipped, fill, hover_icon, icon, icon_button, label, secondary,
};
use crate::app::state::{PlayStatus, Repeat, Route, format_duration};
use crate::app::{App, Ui, ui};

pub fn view(app: &App) -> El<'_> {
    let p = pal();
    let has_track = app.queue.current.is_some();

    // Left: art + title + artists + like.
    let left: El<'_> = match &app.queue.current {
        Some(t) => {
            let liked = app.liked_ids.contains(&t.id);
            let title_btn = button(label(
                t.name.as_str(),
                M.font_md,
                p.text_primary,
                theme::FONT_SEMIBOLD,
            ))
            .padding(0)
            .style(theme::link(false))
            .on_press_maybe(
                t.album
                    .as_ref()
                    .map(|a| ui(Ui::Navigate(Route::Album(a.id.clone())))),
            );
            row![
                art(app, t.images(), 56.0, false, Icon::Music),
                column![
                    clipped(title_btn),
                    artist_links(&t.artists, M.font_xs + 1.0, true)
                ]
                .spacing(4)
                .width(200),
                button(if liked {
                    icon(Icon::HeartFilled, 16.0, p.accent)
                } else {
                    hover_icon(Icon::Heart, 16.0, p.text_secondary, p.text_primary)
                })
                .padding(8)
                .style(theme::icon_button)
                .on_press(ui(Ui::ToggleLike(t.clone()))),
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into()
        }
        None => fill(),
    };

    // Center: transport + seek bar.
    let playing = matches!(
        app.playback.status,
        PlayStatus::Playing | PlayStatus::Loading
    );
    let play = button(
        container(icon(
            if playing { Icon::Pause } else { Icon::Play },
            16.0,
            Color::BLACK,
        ))
        .center_x(32)
        .center_y(32),
    )
    .width(32)
    .height(32)
    .padding(0)
    .style(theme::play_white)
    .on_press_maybe(has_track.then(|| ui(Ui::TogglePlay)));
    let repeat_icon = if app.queue.repeat == Repeat::Track {
        Icon::RepeatOne
    } else {
        Icon::Repeat
    };
    let enabled = |m| has_track.then_some(m);
    let transport = row![
        icon_button(
            Icon::Shuffle,
            16.0,
            Some(ui(Ui::ToggleShuffle)),
            app.queue.shuffle
        ),
        icon_button(Icon::SkipBack, 16.0, enabled(ui(Ui::Previous)), false),
        play,
        icon_button(Icon::SkipForward, 16.0, enabled(ui(Ui::Next)), false),
        icon_button(
            repeat_icon,
            16.0,
            Some(ui(Ui::CycleRepeat)),
            app.queue.repeat != Repeat::Off
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let dur = app.playback.duration_ms;
    let pos = app.playback.position(app.now);
    let frac = if dur > 0 {
        pos as f32 / dur as f32
    } else {
        0.0
    };
    let seek = row![
        container(secondary(format_duration(pos), M.font_xs + 1.0))
            .width(40)
            .align_x(Alignment::End),
        slider(0.0..=1.0, frac, |v| ui(Ui::SeekPreview(v)))
            .step(0.001_f32)
            .on_release(ui(Ui::SeekCommit))
            .style(theme::progress_slider)
            .height(12)
            .width(Length::Fill),
        secondary(format_duration(dur), M.font_xs + 1.0).width(40),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let center = column![transport, seek]
        .spacing(4)
        .align_x(Alignment::Center)
        .max_width(722);

    // Right: queue toggle + volume.
    let vol = app.playback.effective_volume();
    let vol_icon = if vol <= 0.0 {
        Icon::VolumeMute
    } else if vol < 0.5 {
        Icon::VolumeLow
    } else {
        Icon::Volume
    };
    let right = row![
        icon_button(Icon::Lyrics, 16.0, None, false),
        icon_button(Icon::Queue, 16.0, Some(ui(Ui::ToggleQueue)), app.show_queue),
        icon_button(Icon::Device, 16.0, None, false),
        icon_button(vol_icon, 16.0, Some(ui(Ui::ToggleMute)), false),
        slider(0.0..=1.0, vol, |v| ui(Ui::Volume(v)))
            .step(0.01_f32)
            .on_release(ui(Ui::VolumeCommit))
            .style(theme::progress_slider)
            .height(12)
            .width(93),
    ]
    .spacing(0)
    .align_y(Alignment::Center);

    container(
        row![
            container(left).width(Length::FillPortion(3)).clip(true),
            container(center)
                .width(Length::FillPortion(4))
                .center_x(Length::FillPortion(4)),
            container(right)
                .width(Length::FillPortion(3))
                .align_x(Alignment::End),
        ]
        .align_y(Alignment::Center)
        .padding([0, 8]),
    )
    .height(M.player_height)
    .width(Length::Fill)
    .align_y(Alignment::Center)
    .into()
}
