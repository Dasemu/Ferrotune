//! Views: pure functions `&App -> Element<Message>`.

pub mod icons;
pub mod theme;

use iced::Element;
use iced::widget::{container, text};

use crate::app::{App, Message};

pub fn view(app: &App) -> Element<'_, Message> {
    app.mark_frame();
    container(text("Ferrotune")).style(theme::app_bg).into()
}
