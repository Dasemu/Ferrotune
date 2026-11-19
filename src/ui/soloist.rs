//! Spotify Soloist: the setup card (top of the main panel) and the rows of
//! Settings > Playback.

use iced::widget::{button, column, container, row, text_input};
use iced::{Alignment, Length, Padding};

use super::icons::Icon;
use super::theme::{self, FONT, FONT_BOLD, M, pal};
use super::widgets::{El, icon, label, secondary};
use crate::app::soloist::SoloistUi;
use crate::app::{App, Ui, ui};
use crate::net::soloist::EngineStatus;
use crate::net::soloist::archive::format_date;

fn msg(u: SoloistUi) -> crate::app::Message {
    ui(Ui::Soloist(u))
}

fn primary_button<'a>(text: &'a str, m: Option<SoloistUi>) -> El<'a> {
    button(label(text, M.font_sm, pal().accent_text, FONT_BOLD))
        .padding(Padding::from([6, 14]))
        .style(theme::pill_primary)
        .on_press_maybe(m.map(msg))
        .into()
}

fn outline_button<'a>(text: &'a str, m: SoloistUi) -> El<'a> {
    button(label(text, M.font_sm, pal().text_primary, FONT_BOLD))
        .padding(Padding::from([6, 14]))
        .style(theme::pill_outline)
        .on_press(msg(m))
        .into()
}

fn wrapped<'a>(s: String) -> El<'a> {
    secondary(s, M.font_sm)
        .wrapping(iced::widget::text::Wrapping::Word)
        .width(Length::Fill)
        .into()
}

/// The API key field: masked, pasted from the dashboard.
fn key_row(app: &App) -> El<'_> {
    let value = app.soloist.key_input.as_ref().map_or("", |s| s.expose());
    let can_save = !value.trim().is_empty();
    row![
        outline_button("Get API key", SoloistUi::OpenKeyPage),
        text_input("Paste your Spotify Soloist API key", value)
            .secure(true)
            .on_input(|v| msg(SoloistUi::KeyInput(crate::auth::secrets::Secret::new(v))))
            .on_submit(msg(SoloistUi::SaveKey))
            .padding(Padding::from([8, 12]))
            .size(M.font_md)
            .width(Length::Fill)
            .style(theme::search_input),
        primary_button("Save", can_save.then_some(SoloistUi::SaveKey)),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

/// Setup card: shown while Soloist needs the user (key, pairing, error),
/// or when opened from Settings / the "audio key denied" path.
pub fn setup_card(app: &App) -> Option<El<'_>> {
    if !app.soloist_card() {
        return None;
    }
    let p = pal();
    let s = &app.soloist;
    let device = app.settings_device_name();
    let (title, body, actions): (String, String, El<'_>) = if s.wants_key()
        || (!app.soloist_enabled() && !s.has_key())
    {
        (
            "Play music on this computer".into(),
            "Spotify only plays audio on this PC through its official player, Spotify Soloist. \
             Ferrotune downloads it from Spotify and runs it for you. It needs your personal \
             API key (Premium): create it once in Spotify for Developers and paste it here."
                .into(),
            key_row(app),
        )
    } else {
        match &s.status {
            EngineStatus::NeedsPairing | EngineStatus::Pairing => (
                "One last step".into(),
                format!(
                    "Open Spotify on your phone, tap the devices icon and choose \u{201c}{device}\u{201d} once. \
                     Ferrotune takes over from there."
                ),
                row![].into(),
            ),
            EngineStatus::Error(_) | EngineStatus::Expired => (
                "Spotify Soloist is not running".into(),
                s.status.label(),
                row![
                    primary_button("Try again", Some(SoloistUi::UpdateNow)),
                    outline_button("Use the built-in player", SoloistUi::Enable(false)),
                ]
                .spacing(8)
                .into(),
            ),
            EngineStatus::Unsupported => (
                "Spotify Soloist".into(),
                "Spotify does not publish Soloist for this computer yet.".into(),
                outline_button("Use the built-in player", SoloistUi::Enable(false)),
            ),
            EngineStatus::Ready => (
                "Spotify Soloist is ready".into(),
                format!("Music plays on this computer as \u{201c}{device}\u{201d}."),
                row![].into(),
            ),
            other => (
                "Setting up Spotify Soloist".into(),
                format!("{} Downloading from Spotify the first time.", other.label()),
                row![].into(),
            ),
        }
    };
    let close = button(icon(Icon::Close, 14.0, p.text_secondary))
        .padding(6)
        .style(theme::icon_button)
        .on_press(msg(SoloistUi::CloseSetup));
    let busy = s.status.is_busy() || matches!(s.status, EngineStatus::NeedsPairing);
    let head = row![
        icon(
            if busy { Icon::Loader } else { Icon::Device },
            18.0,
            p.accent
        ),
        label(title, M.font_lg, p.text_primary, FONT_BOLD).width(Length::Fill),
        close,
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    Some(
        container(column![head, wrapped(body), actions].spacing(10))
            .padding(Padding::from([14, 16]))
            .width(Length::Fill)
            .style(theme::setup_card)
            .into(),
    )
}

/// Rows for Settings > Playback. `row` lays out a title, a hint and a
/// control like the other settings.
pub fn settings_rows<'a>(
    app: &'a App,
    row_fn: fn(&'a str, Option<&'a str>, El<'a>) -> El<'a>,
    switch: El<'a>,
) -> Vec<El<'a>> {
    let s = &app.soloist;
    let mut rows = vec![row_fn(
        "Play with Spotify Soloist",
        Some(
            "Spotify's official player plays the audio on this computer (downloaded from Spotify, needs your API key).",
        ),
        switch,
    )];
    if !app.soloist_enabled() {
        return rows;
    }
    let mut status = s.status.label();
    if let Some(m) = s.memory {
        status.push_str(&format!(" · {:.0} MB", m as f64 / (1024.0 * 1024.0)));
    }
    rows.push(row_fn(
        "Status",
        None,
        label(status, M.font_sm, pal().text_secondary, FONT).into(),
    ));
    let version = match &s.build {
        Some(b) => format!(
            "{} · expires {}",
            b.version.as_deref().unwrap_or("unknown version"),
            format_date(b.expires_unix)
        ),
        None => "Not installed".into(),
    };
    rows.push(row_fn(
        "Version",
        Some("Builds expire after 90 days; Ferrotune updates them from Spotify."),
        row![
            secondary(version, M.font_sm),
            outline_button("Update now", SoloistUi::UpdateNow)
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into(),
    ));
    rows.push(row_fn(
        "API key",
        Some("Kept in the system keyring."),
        outline_button("Replace API key", SoloistUi::ReplaceKey),
    ));
    rows.push(row_fn(
        "Pairing",
        Some("Choose this computer once in the Spotify app on your phone."),
        outline_button("Re-pair", SoloistUi::Repair),
    ));
    let output = text_input("System default", &app.settings.soloist_output)
        .on_input(|v| msg(SoloistUi::Output(v)))
        .on_submit(msg(SoloistUi::OutputCommit))
        .padding(Padding::from([8, 12]))
        .size(M.font_md)
        .width(260)
        .style(theme::search_input);
    rows.push(row_fn(
        "Output device",
        Some("PipeWire node name or id. Press Enter to save."),
        output.into(),
    ));
    rows
}
