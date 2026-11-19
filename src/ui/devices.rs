//! Spotify Connect device picker ("Connect to a device") and the green
//! "Playing on …" strip, laid out like the official desktop client.

use iced::widget::{Space, button, column, container, mouse_area, row, stack};
use iced::{Alignment, Length, Padding};

use super::icons::Icon;
use super::theme::{self, FONT, FONT_BOLD, M, pal};
use super::widgets::{El, icon, label, secondary};
use crate::app::{App, Ui, ui};
use crate::net::connect_ctl::DeviceKind;

const PICKER_WIDTH: f32 = 320.0;

fn kind_icon(k: DeviceKind) -> Icon {
    match k {
        DeviceKind::Computer => Icon::Device,
        DeviceKind::Smartphone | DeviceKind::Tablet => Icon::Smartphone,
        DeviceKind::Speaker | DeviceKind::Car | DeviceKind::Other => Icon::Speaker,
        DeviceKind::Tv => Icon::Tv,
    }
}

fn device_row<'a>(name: String, sub: Option<String>, ic: Icon, active: bool, msg: Ui) -> El<'a> {
    let p = pal();
    let colour = if active { p.accent } else { p.text_primary };
    let mut text = column![label(name, M.font_md, colour, FONT)].spacing(2);
    if let Some(s) = sub {
        text = text.push(secondary(s, M.font_xs + 1.0));
    }
    button(
        row![icon(ic, 24.0, colour), text]
            .spacing(12)
            .align_y(Alignment::Center),
    )
    .padding(Padding::from([10, 12]))
    .width(Length::Fill)
    .style(theme::menu_item)
    .on_press(ui(msg))
    .into()
}

/// The picker popup above the device button (bottom right).
pub fn picker(app: &App) -> El<'_> {
    let p = pal();
    let remote = app.remote_device();
    let soloist_here = app.soloist_playing_here();
    let local_active = remote.is_none() || soloist_here;
    let mut col = column![
        container(label(
            "Current device",
            M.font_md,
            p.text_primary,
            FONT_BOLD
        ))
        .padding(Padding::from([8, 12])),
        device_row(
            "This computer".into(),
            Some(if app.soloist_enabled() {
                format!(
                    "{} · Spotify Soloist ({})",
                    app.settings_device_name(),
                    app.soloist.status.label().to_lowercase()
                )
            } else if app.local_denied {
                "Spotify does not allow playback here".into()
            } else {
                app.settings_device_name()
            }),
            Icon::Device,
            local_active,
            Ui::PlayOn(None),
        ),
    ]
    .spacing(0);
    // Soloist is "This computer", not another device.
    let devices: Vec<_> = app
        .cluster
        .as_ref()
        .map(|c| c.devices.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|d| !app.is_soloist_id(&d.id))
        .collect();
    col = col.push(
        container(label(
            "Select another device",
            M.font_sm,
            p.text_secondary,
            FONT_BOLD,
        ))
        .padding(Padding {
            top: 12.0,
            right: 12.0,
            bottom: 4.0,
            left: 12.0,
        }),
    );
    if devices.is_empty() {
        col = col.push(
            container(secondary(
                if app.cluster.is_none() {
                    "Looking for devices…"
                } else {
                    "No other devices found. Open Spotify on your phone or computer."
                },
                M.font_sm,
            ))
            .padding(Padding::from([8, 12])),
        );
    }
    for d in devices {
        let active = remote == Some(d.id.as_str());
        col = col.push(device_row(
            d.name.clone(),
            active.then(|| "Playing".to_owned()),
            kind_icon(d.kind),
            active,
            Ui::PlayOn(Some((d.id.clone(), d.name.clone()))),
        ));
    }
    let panel = container(col.padding(8))
        .width(PICKER_WIDTH)
        .style(theme::menu_panel);
    let backdrop = mouse_area(
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .on_press(ui(Ui::ToggleDevicePicker));
    let positioned = container(panel)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::End)
        .align_y(Alignment::End)
        .padding(Padding {
            bottom: M.player_height + 4.0,
            right: 120.0,
            ..Padding::ZERO
        });
    stack![backdrop, positioned].into()
}

/// Green strip under the player while another device plays.
pub fn playing_on(app: &App) -> Option<El<'_>> {
    if app.soloist_playing_here() {
        return None;
    }
    let name = app.remote.as_ref()?.device_name()?;
    Some(
        container(
            row![
                icon(Icon::Speaker, 14.0, pal().accent_text),
                label(
                    format!("Playing on {name}"),
                    M.font_sm,
                    pal().accent_text,
                    FONT_BOLD
                ),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([4, 16]))
        .width(Length::Fill)
        .align_x(Alignment::End)
        .style(theme::connect_strip)
        .into(),
    )
}

/// Persistent playback problem (audio key denied, too many unavailable
/// songs): one banner with the way out, never a looping toast.
pub fn block_banner(app: &App) -> Option<El<'_>> {
    let b = app.playback_block.as_ref()?;
    let p = pal();
    let mut actions = row![].spacing(8).align_y(Alignment::Center);
    if b.reason.is_key_denied() {
        if let Some(dev) = app.suggested_device() {
            let name = dev.1.clone();
            actions = actions.push(
                button(label(
                    format!("Play on {name}"),
                    M.font_sm,
                    pal().accent_text,
                    FONT_BOLD,
                ))
                .padding(Padding::from([6, 14]))
                .style(theme::pill_primary)
                .on_press(ui(Ui::PlayOn(Some(dev)))),
            );
        }
        actions = actions.push(
            button(label("Choose device", M.font_sm, p.text_primary, FONT_BOLD))
                .padding(Padding::from([6, 14]))
                .style(theme::pill_outline)
                .on_press(ui(Ui::ToggleDevicePicker)),
        );
    }
    actions = actions.push(
        button(icon(Icon::Close, 14.0, p.text_secondary))
            .padding(6)
            .style(theme::icon_button)
            .on_press(ui(Ui::DismissBanner)),
    );
    Some(
        container(
            row![
                icon(Icon::Alert, 16.0, p.text_primary),
                label(b.message.as_str(), M.font_sm, p.text_primary, FONT)
                    .wrapping(iced::widget::text::Wrapping::Word)
                    .width(Length::Fill),
                actions,
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([8, 12]))
        .width(Length::Fill)
        .style(theme::error_banner)
        .into(),
    )
}
