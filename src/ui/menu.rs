//! Context menu and modal dialogs, drawn above the main layout.

use iced::widget::{
    Space, button, column, container, mouse_area, row, scrollable, stack, text_input,
};
use iced::{Alignment, Element, Length, Padding, Size};

use super::context_area::{ContextArea, Trigger};
use super::theme::{self, FONT, FONT_BOLD, M, MENU_ROW, MENU_WIDTH, pal};
use super::widgets::{El, bold, fill, icon, label, secondary, semibold};
use crate::app::menu::{ContextMenu, Dialog, Entry, MenuTarget, entries, place};
use crate::app::shortcuts::HELP;
use crate::app::{App, Done, Message, Ui, ui};
use crate::ui::icons::Icon;

pub const DIALOG_INPUT_ID: &str = "dialog-input";
const PICKER_MAX: usize = 7;

/// Open a context menu for `target` on a right click anywhere on `content`.
pub fn right_click<'a>(content: impl Into<El<'a>>, target: MenuTarget) -> El<'a> {
    ContextArea::new(content, Trigger::Right, move |p| {
        ui(Ui::OpenMenu(target.clone(), p))
    })
    .into()
}

/// Open a context menu for `target` on a left click (the "..." buttons).
pub fn click_menu<'a>(content: impl Into<El<'a>>, target: MenuTarget) -> El<'a> {
    ContextArea::new(content, Trigger::Left, move |p| {
        ui(Ui::OpenMenu(target.clone(), p))
    })
    .into()
}

fn item<'a>(text: impl Into<String>, msg: Message) -> El<'a> {
    button(
        container(label(text.into(), M.font_md, pal().text_primary, FONT)).center_y(Length::Fill),
    )
    .padding(Padding::from([0, 12]))
    .height(MENU_ROW)
    .width(Length::Fill)
    .style(theme::menu_item)
    .on_press(msg)
    .into()
}

fn separator<'a>() -> El<'a> {
    container(Space::new().height(1))
        .width(Length::Fill)
        .style(theme::separator)
        .into()
}

fn menu_height(app: &App, m: &ContextMenu) -> f32 {
    let es = entries(&m.target, false);
    let rows = es.iter().filter(|e| !matches!(e, Entry::Separator)).count() as f32;
    let seps = es.len() as f32 - rows;
    let picker = if m.picker {
        app.editable_playlists().len().clamp(1, PICKER_MAX) as f32 * MENU_ROW
    } else {
        0.0
    };
    rows * MENU_ROW + seps * 9.0 + picker + 16.0
}

fn context_menu<'a>(app: &'a App, m: &'a ContextMenu) -> El<'a> {
    let liked = match &m.target {
        MenuTarget::Track { track, .. } => app.liked_ids.contains(&track.id),
        _ => false,
    };
    let mut col = column![].spacing(0);
    for e in entries(&m.target, liked) {
        match e {
            Entry::Action { label, msg } => {
                col = col.push(item(label, ui(Entry::message(&msg))));
            }
            Entry::Separator => {
                col = col.push(container(separator()).padding(Padding::from([4, 0])));
            }
            Entry::AddToPlaylist => {
                let arrow = if m.picker {
                    Icon::ChevronLeft
                } else {
                    Icon::ChevronRight
                };
                col = col.push(
                    button(
                        container(
                            row![
                                label_text("Add to playlist"),
                                fill(),
                                icon(arrow, 16.0, pal().text_secondary)
                            ]
                            .align_y(Alignment::Center),
                        )
                        .center_y(Length::Fill),
                    )
                    .padding(Padding::from([0, 12]))
                    .height(MENU_ROW)
                    .width(Length::Fill)
                    .style(theme::menu_item)
                    .on_press(ui(Ui::ToggleMenuPicker)),
                );
                if m.picker {
                    col = col.push(picker(app, &m.target));
                }
            }
        }
    }
    let size = Size::new(MENU_WIDTH, menu_height(app, m));
    let at = place(m.at, app.window_size, size);
    let panel = container(col.padding(8))
        .width(MENU_WIDTH)
        .style(theme::menu_panel);
    let positioned = container(panel)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(Padding {
            top: at.y,
            left: at.x,
            ..Padding::ZERO
        });
    let backdrop = mouse_area(Space::new().width(Length::Fill).height(Length::Fill))
        .on_press(ui(Ui::CloseOverlay))
        .on_right_press(ui(Ui::CloseOverlay))
        // Swallow the wheel so the page does not scroll under an open menu.
        .on_scroll(|_| Message::Done(Done::Noop));
    stack![backdrop, positioned].into()
}

fn label_text<'a>(s: &'a str) -> El<'a> {
    label(s, M.font_md, pal().text_primary, FONT).into()
}

fn picker<'a>(app: &'a App, target: &MenuTarget) -> El<'a> {
    let MenuTarget::Track { track, .. } = target else {
        return Space::new().into();
    };
    let lists = app.editable_playlists();
    if lists.is_empty() {
        return container(secondary("You have no playlists yet", M.font_sm))
            .padding(Padding::from([8, 24]))
            .height(MENU_ROW)
            .into();
    }
    let mut col = column![].spacing(0);
    for p in lists {
        col = col.push(
            button(
                container(label(p.name.as_str(), M.font_md, pal().text_primary, FONT))
                    .center_y(Length::Fill),
            )
            .padding(Padding {
                left: 24.0,
                right: 12.0,
                ..Padding::ZERO
            })
            .height(MENU_ROW)
            .width(Length::Fill)
            .style(theme::menu_item)
            .on_press(ui(Ui::AddToPlaylist(p.id.clone(), track.clone()))),
        );
    }
    scrollable(col)
        .style(theme::thin_scroll)
        .height(Length::Shrink)
        .into()
}

fn pill<'a>(text: &'a str, primary: bool, msg: Message) -> El<'a> {
    button(label(
        text,
        M.font_md,
        if primary {
            pal().accent_text
        } else {
            pal().text_primary
        },
        FONT_BOLD,
    ))
    .padding(Padding::from([10, 28]))
    .style(if primary {
        theme::pill_primary
    } else {
        theme::pill_outline
    })
    .on_press(msg)
    .into()
}

fn keycaps<'a>(keys: &'a str) -> El<'a> {
    // "Ctrl + L  or  Ctrl + K": alternatives are separated by two spaces.
    let mut r = row![].spacing(8).align_y(Alignment::Center);
    for (i, alt) in keys.split("  or  ").enumerate() {
        if i > 0 {
            r = r.push(secondary("or", M.font_sm));
        }
        let mut combo = row![].spacing(4).align_y(Alignment::Center);
        for k in alt.split(" + ") {
            combo = combo.push(
                container(label(k, M.font_sm, pal().text_primary, FONT))
                    .padding(Padding::from([2, 8]))
                    .style(theme::keycap),
            );
        }
        r = r.push(combo);
    }
    r.into()
}

fn shortcuts_dialog<'a>() -> El<'a> {
    let mut col = column![bold("Keyboard shortcuts", M.font_xl)].spacing(12);
    for (section, rows) in HELP {
        col = col.push(semibold(*section, M.font_lg));
        for (keys, what) in *rows {
            col = col.push(
                row![
                    label(*what, M.font_md, pal().text_primary, FONT),
                    fill(),
                    keycaps(keys)
                ]
                .align_y(Alignment::Center),
            );
        }
    }
    col = col
        .push(row![fill(), pill("Close", false, ui(Ui::CloseOverlay))].align_y(Alignment::Center));
    container(col.width(480))
        .padding(24)
        .style(theme::dialog_card)
        .into()
}

fn rename_dialog<'a>(name: &'a str) -> El<'a> {
    let input = text_input("Playlist name", name)
        .id(DIALOG_INPUT_ID)
        .on_input(|s| ui(Ui::DialogInput(s)))
        .on_submit(ui(Ui::DialogSubmit))
        .padding(12)
        .size(M.font_lg)
        .style(theme::search_input);
    let save: Message = if name.trim().is_empty() {
        Message::Done(Done::Noop)
    } else {
        ui(Ui::DialogSubmit)
    };
    container(
        column![
            bold("Edit details", M.font_xl),
            input,
            row![
                fill(),
                pill("Cancel", false, ui(Ui::CloseOverlay)),
                pill("Save", true, save)
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        ]
        .spacing(20)
        .width(420),
    )
    .padding(24)
    .style(theme::dialog_card)
    .into()
}

fn delete_dialog<'a>(name: &'a str, owned: bool) -> El<'a> {
    let (title, body, action) = if owned {
        (
            "Delete from Your Library?",
            format!("This will delete {name} from Your Library."),
            "Delete",
        )
    } else {
        (
            "Remove from Your Library?",
            format!("{name} will be removed from Your Library."),
            "Remove",
        )
    };
    container(
        column![
            bold(title, M.font_xl),
            secondary(body, M.font_md),
            row![
                fill(),
                pill("Cancel", false, ui(Ui::CloseOverlay)),
                pill(action, true, ui(Ui::DialogSubmit))
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        ]
        .spacing(20)
        .width(420),
    )
    .padding(24)
    .style(theme::dialog_card)
    .into()
}

fn dialog<'a>(d: &'a Dialog) -> El<'a> {
    let card: El<'a> = match d {
        Dialog::Shortcuts => shortcuts_dialog(),
        Dialog::Rename { name, .. } => rename_dialog(name),
        Dialog::ConfirmDelete { name, owned, .. } => delete_dialog(name, *owned),
    };
    // Clicks on the card must not reach the scrim underneath.
    let card = mouse_area(card).on_press(Message::Done(Done::Noop));
    let centered = container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill);
    let scrim = mouse_area(
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::scrim),
    )
    .on_press(ui(Ui::CloseOverlay));
    stack![scrim, centered].into()
}

/// Overlay layers above the main layout, if any.
pub fn overlay(app: &App) -> Option<Element<'_, Message>> {
    if let Some(d) = &app.dialog {
        return Some(dialog(d));
    }
    app.menu.as_ref().map(|m| context_menu(app, m))
}
