//! Shared building blocks: icons, artwork, text, cards, headers and the
//! track table.

use iced::widget::{Space, button, column, container, image, mouse_area, row, space, svg, text};
use iced::{Alignment, Color, Element, Font, Length, Padding};

use super::icons::{Icon, handle};
use super::theme::{self, FONT, FONT_BOLD, FONT_SEMIBOLD, M, pal};
use crate::app::state::{Route, format_duration};
use crate::app::{App, Message, TrackSource, Ui, ui};
use crate::net::model::{ArtistRef, Image, Track};

pub type El<'a> = Element<'a, Message>;

pub fn icon<'a>(i: Icon, size: f32, color: Color) -> El<'a> {
    svg(handle(i))
        .width(size)
        .height(size)
        .style(theme::svg_color(color))
        .into()
}

/// Icon that brightens when hovered.
pub fn hover_icon<'a>(i: Icon, size: f32, normal: Color, hover: Color) -> El<'a> {
    svg(handle(i))
        .width(size)
        .height(size)
        .style(theme::svg_hover(normal, hover))
        .into()
}

pub fn icon_button<'a>(i: Icon, size: f32, msg: Option<Message>, active: bool) -> El<'a> {
    let p = pal();
    let normal = if active { p.accent } else { p.text_secondary };
    let hover = if active {
        theme::mix(p.accent, Color::WHITE, 0.2)
    } else {
        p.text_primary
    };
    let content: El<'a> = if msg.is_some() {
        hover_icon(i, size, normal, hover)
    } else {
        icon(i, size, p.text_muted)
    };
    button(content)
        .padding(8)
        .style(theme::icon_button)
        .on_press_maybe(msg)
        .into()
}

pub fn label<'a>(
    s: impl text::IntoFragment<'a>,
    size: f32,
    color: Color,
    font: Font,
) -> text::Text<'a> {
    text(s)
        .size(size)
        .color(color)
        .font(font)
        .wrapping(text::Wrapping::None)
}

pub fn primary<'a>(s: impl text::IntoFragment<'a>, size: f32) -> text::Text<'a> {
    label(s, size, pal().text_primary, FONT)
}

pub fn secondary<'a>(s: impl text::IntoFragment<'a>, size: f32) -> text::Text<'a> {
    label(s, size, pal().text_secondary, FONT)
}

pub fn bold<'a>(s: impl text::IntoFragment<'a>, size: f32) -> text::Text<'a> {
    label(s, size, pal().text_primary, FONT_BOLD)
}

pub fn semibold<'a>(s: impl text::IntoFragment<'a>, size: f32) -> text::Text<'a> {
    label(s, size, pal().text_primary, FONT_SEMIBOLD)
}

/// Clip a widget horizontally so long titles don't wrap or overflow.
pub fn clipped<'a>(e: impl Into<El<'a>>) -> El<'a> {
    container(e).width(Length::Fill).clip(true).into()
}

/// Album art / avatar with a placeholder while it loads.
pub fn art<'a>(app: &App, images: &[Image], size: f32, round: bool, fallback: Icon) -> El<'a> {
    let px = if size > 64.0 { 300 } else { 64 };
    match app.image(images, px) {
        Some(h) => image(h)
            .width(size)
            .height(size)
            .content_fit(iced::ContentFit::Cover)
            .border_radius(if round { size / 2.0 } else { 4.0 })
            .into(),
        None => container(icon(fallback, size * 0.4, pal().text_muted))
            .width(size)
            .height(size)
            .center_x(size)
            .center_y(size)
            .style(theme::art_placeholder(round, size))
            .into(),
    }
}

/// The gradient "Liked Songs" tile.
pub fn liked_art<'a>(size: f32) -> El<'a> {
    container(icon(Icon::HeartFilled, size * 0.4, Color::WHITE))
        .width(size)
        .height(size)
        .center_x(size)
        .center_y(size)
        .style(theme::liked_tile(4.0))
        .into()
}

pub fn hspace<'a>(w: f32) -> El<'a> {
    Space::new().width(w).into()
}

pub fn vspace<'a>(h: f32) -> El<'a> {
    Space::new().height(h).into()
}

pub fn fill<'a>() -> El<'a> {
    space::horizontal().into()
}

/// Comma separated, individually clickable artist names.
pub fn artist_links<'a>(artists: &'a [ArtistRef], size: f32, muted: bool) -> El<'a> {
    let mut r = row![].spacing(0).align_y(Alignment::Center);
    for (i, a) in artists.iter().enumerate() {
        if i > 0 {
            r = r.push(label(", ", size, pal().text_secondary, FONT));
        }
        let t = label(a.name.as_str(), size, Color::WHITE, FONT);
        let b = button(t).padding(0).style(theme::link(muted));
        r = r.push(if a.id.is_empty() {
            b
        } else {
            b.on_press(ui(Ui::Navigate(Route::Artist(a.id.clone()))))
        });
    }
    clipped(r)
}

/// A square (or round) card: art, title, subtitle.
pub fn card<'a>(
    app: &App,
    images: &[Image],
    title: &'a str,
    subtitle: String,
    round: bool,
    fallback: Icon,
    on_press: Message,
) -> El<'a> {
    let content = column![
        art(app, images, M.art_card, round, fallback),
        vspace(4.0),
        clipped(semibold(title, M.font_md)),
        clipped(secondary(subtitle, M.font_sm)),
    ]
    .spacing(4)
    .width(M.art_card);
    button(content)
        .padding(12)
        .style(theme::card)
        .on_press(on_press)
        .into()
}

pub fn section<'a>(title: &'a str, body: El<'a>) -> El<'a> {
    column![bold(title, M.font_xl), vspace(8.0), body]
        .spacing(0)
        .into()
}

/// Up to `n` items in a non-wrapping row.
pub fn card_row<'a>(cards: Vec<El<'a>>) -> El<'a> {
    let mut r = row![].spacing(0);
    for c in cards {
        r = r.push(c);
    }
    container(r).width(Length::Fill).clip(true).into()
}

pub fn wrap_grid<'a>(cards: Vec<El<'a>>) -> El<'a> {
    let mut r = row![].spacing(0);
    for c in cards {
        r = r.push(c);
    }
    r.wrap().into()
}

/// Large green play/pause button.
pub fn big_play<'a>(playing: bool, msg: Message) -> El<'a> {
    let i = if playing { Icon::Pause } else { Icon::Play };
    button(
        container(icon(i, 22.0, Color::BLACK))
            .center_x(56)
            .center_y(56),
    )
    .width(56)
    .height(56)
    .padding(0)
    .style(theme::play_green)
    .on_press(msg)
    .into()
}

/// Gradient header shared by playlist / album / artist / liked views.
pub fn header<'a>(
    tint: Color,
    art: El<'a>,
    kind: &'a str,
    title: String,
    description: Option<String>,
    meta: El<'a>,
) -> El<'a> {
    let title_size = match title.chars().count() {
        0..=14 => M.font_hero,
        15..=24 => 48.0,
        _ => 32.0,
    };
    let mut info = column![
        semibold(kind, M.font_sm),
        container(
            label(title, title_size, Color::WHITE, FONT_BOLD).wrapping(text::Wrapping::WordOrGlyph)
        )
        .padding(Padding::from([4, 0])),
    ]
    .spacing(4);
    if let Some(d) = description.filter(|d| !d.is_empty()) {
        info = info
            .push(label(d, M.font_sm, pal().text_secondary, FONT).wrapping(text::Wrapping::Word));
    }
    info = info.push(meta);
    let content = row![
        art,
        column![space::vertical(), info]
            .height(M.art_header)
            .width(Length::Fill)
    ]
    .spacing(24)
    .align_y(Alignment::End);
    container(content)
        .padding(Padding {
            top: 64.0,
            right: 24.0,
            bottom: 24.0,
            left: 24.0,
        })
        .width(Length::Fill)
        .style(theme::gradient_header(
            tint,
            theme::mix(tint, pal().bg_panel, 0.55),
        ))
        .into()
}

/// The darker band under the header holding the play button.
pub fn action_band<'a>(tint: Color, content: El<'a>) -> El<'a> {
    container(content)
        .padding(Padding::from([24, 24]))
        .width(Length::Fill)
        .style(theme::gradient_header(
            theme::mix(tint, pal().bg_panel, 0.75),
            pal().bg_panel,
        ))
        .into()
}

pub fn meta_line<'a>(parts: Vec<El<'a>>) -> El<'a> {
    let mut r = row![].spacing(4).align_y(Alignment::Center);
    for (i, p) in parts.into_iter().enumerate() {
        if i > 0 {
            r = r.push(secondary("•", M.font_sm));
        }
        r = r.push(p);
    }
    r.into()
}

/// "2025-01-10T…" → "Jan 10, 2025"
pub fn format_date(iso: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let date = iso.get(..10).unwrap_or(iso);
    let mut it = date.split('-');
    match (
        it.next(),
        it.next().and_then(|m| m.parse::<usize>().ok()),
        it.next().and_then(|d| d.parse::<u32>().ok()),
    ) {
        (Some(y), Some(m), Some(d)) if (1..=12).contains(&m) => {
            format!("{} {d}, {y}", MONTHS[m - 1])
        }
        _ => iso.to_owned(),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TableOpts {
    pub album: bool,
    pub added: bool,
    pub art: bool,
    pub header: bool,
}

const COL_NUM: f32 = 32.0;
const COL_DUR: f32 = 120.0;

fn table_header<'a>(opts: TableOpts) -> El<'a> {
    let p = pal();
    let mut r = row![
        container(secondary("#", M.font_sm))
            .width(COL_NUM)
            .align_x(Alignment::End),
        hspace(16.0),
        container(secondary("Title", M.font_sm)).width(Length::FillPortion(4)),
    ]
    .align_y(Alignment::Center);
    if opts.album {
        r = r.push(container(secondary("Album", M.font_sm)).width(Length::FillPortion(3)));
    }
    if opts.added {
        r = r.push(container(secondary("Date added", M.font_sm)).width(Length::FillPortion(2)));
    }
    r = r.push(
        container(icon(Icon::Clock, 16.0, p.text_secondary))
            .width(COL_DUR)
            .align_x(Alignment::End)
            .padding(Padding::from([0, 40])),
    );
    column![
        container(r).padding(Padding::from([8, 16])),
        container(Space::new().height(1))
            .width(Length::Fill)
            .style(theme::separator),
        vspace(8.0),
    ]
    .into()
}

pub fn track_table<'a>(
    app: &'a App,
    tracks: &'a [Track],
    src: TrackSource,
    opts: TableOpts,
) -> El<'a> {
    let mut col = column![].spacing(0);
    if opts.header {
        col = col.push(table_header(opts));
    }
    let key_prefix = format!("{src:?}");
    for (i, t) in tracks.iter().enumerate() {
        col = col.push(track_row(app, t, i, &src, &key_prefix, opts));
    }
    col.into()
}

fn track_row<'a>(
    app: &'a App,
    t: &'a Track,
    i: usize,
    src: &TrackSource,
    prefix: &str,
    opts: TableOpts,
) -> El<'a> {
    let p = pal();
    let key = format!("{prefix}#{i}");
    let hovered = app.hovered_row.as_deref() == Some(key.as_str());
    let selected = app.selected_row.as_deref() == Some(key.as_str());
    let current = app.now_track().is_some_and(|c| c.uri == t.uri);
    let playing = current && app.now_playback().is_playing();
    let liked = app.liked_ids.contains(&t.id);
    let title_color = if current {
        p.accent
    } else if t.playable {
        p.text_primary
    } else {
        p.text_muted
    };

    let num: El<'a> = if hovered && t.playable {
        let (ic, msg) = if playing {
            (Icon::Pause, ui(Ui::TogglePlay))
        } else {
            (Icon::Play, ui(Ui::PlayFrom(src.clone(), Some(i))))
        };
        button(icon(ic, 14.0, Color::WHITE))
            .padding(0)
            .style(theme::icon_button)
            .on_press(msg)
            .into()
    } else if playing {
        icon(Icon::Volume, 14.0, p.accent)
    } else {
        label(
            (i + 1).to_string(),
            M.font_md,
            if current { p.accent } else { p.text_secondary },
            FONT,
        )
        .into()
    };

    let mut sub = row![].spacing(6).align_y(Alignment::Center);
    if t.explicit {
        sub = sub.push(
            container(label("E", 9.0, Color::BLACK, FONT_BOLD))
                .padding(Padding::from([1, 4]))
                .style(|_: &iced::Theme| container::Style {
                    background: Some(pal().text_secondary.into()),
                    border: iced::Border {
                        radius: 2.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
        );
    }
    sub = sub.push(artist_links(&t.artists, M.font_sm, !(hovered || selected)));

    let mut title_row = row![].spacing(12).align_y(Alignment::Center);
    if opts.art {
        title_row = title_row.push(art(app, t.images(), M.art_row, false, Icon::Music));
    }
    title_row = title_row.push(
        column![
            clipped(label(t.name.as_str(), M.font_lg, title_color, FONT)),
            sub
        ]
        .spacing(2)
        .width(Length::Fill),
    );

    let mut r = row![
        container(num).width(COL_NUM).align_x(Alignment::End),
        hspace(16.0),
        container(title_row).width(Length::FillPortion(4)),
    ]
    .align_y(Alignment::Center);
    if opts.album {
        let album: El<'a> = match &t.album {
            Some(a) => clipped(
                button(label(a.name.as_str(), M.font_sm, Color::WHITE, FONT))
                    .padding(0)
                    .style(theme::link(!(hovered || selected)))
                    .on_press(ui(Ui::Navigate(Route::Album(a.id.clone())))),
            ),
            None => hspace(0.0),
        };
        r = r.push(
            container(album)
                .width(Length::FillPortion(3))
                .padding(Padding::from([0, 8])),
        );
    }
    if opts.added {
        let added = t.added_at.as_deref().map(format_date).unwrap_or_default();
        r = r.push(container(secondary(added, M.font_sm)).width(Length::FillPortion(2)));
    }
    let heart: El<'a> = if liked || hovered {
        button(if liked {
            icon(Icon::HeartFilled, 16.0, p.accent)
        } else {
            hover_icon(Icon::Heart, 16.0, p.text_secondary, p.text_primary)
        })
        .padding(4)
        .style(theme::icon_button)
        .on_press(ui(Ui::ToggleLike(t.clone())))
        .into()
    } else {
        hspace(24.0)
    };
    let enqueue: El<'a> = if hovered && t.playable {
        button(hover_icon(
            Icon::Plus,
            16.0,
            p.text_secondary,
            p.text_primary,
        ))
        .padding(4)
        .style(theme::icon_button)
        .on_press(ui(Ui::Enqueue(t.clone())))
        .into()
    } else {
        hspace(24.0)
    };
    r = r.push(
        row![
            heart,
            container(secondary(format_duration(t.duration_ms), M.font_sm))
                .width(44)
                .align_x(Alignment::End),
            enqueue,
        ]
        .spacing(8)
        .width(COL_DUR)
        .align_y(Alignment::Center),
    );

    let body = container(r)
        .padding(Padding::from([0, 16]))
        .height(M.row_height)
        .align_y(Alignment::Center)
        .style(move |th: &iced::Theme| {
            let s = theme::track_row(hovered, selected)(th, button::Status::Active);
            container::Style {
                background: s.background,
                border: s.border,
                ..Default::default()
            }
        });
    let mut area = mouse_area(body)
        .on_enter(ui(Ui::HoverRow(Some(key.clone()))))
        .on_press(ui(Ui::SelectRow(key)));
    if t.playable {
        area = area.on_double_click(ui(Ui::PlayFrom(src.clone(), Some(i))));
    }
    area.into()
}

/// Centered spinner-ish loading text.
pub fn loading<'a>() -> El<'a> {
    container(
        row![
            icon(Icon::Loader, 20.0, pal().text_secondary),
            secondary("Loading…", M.font_md)
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .center_x(Length::Fill)
    .padding(64)
    .into()
}

pub fn error_box<'a>(msg: String) -> El<'a> {
    container(
        row![
            icon(Icon::Alert, 20.0, pal().danger),
            secondary(msg, M.font_md)
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .center_x(Length::Fill)
    .padding(64)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(format_date("2025-01-10T10:00:00Z"), "Jan 10, 2025");
        assert_eq!(format_date("garbage"), "garbage");
    }
}
