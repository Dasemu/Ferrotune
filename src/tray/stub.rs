//! Windows / macOS: no tray yet. The stream reports "unavailable" at once,
//! so the window logic falls back to "closing the window quits".

use futures::Stream;

use super::model::Event;

pub fn events() -> impl Stream<Item = Event> + Send + 'static {
    futures::stream::iter([Event::Unavailable])
}
