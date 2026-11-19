//! Window lifecycle and system tray glue: turns `window::Action`s into
//! iced window tasks and keeps the tray in sync with the app state.

use iced::Task;

use super::App;
use super::message::{Done, Message, Ui};
use super::settings::SettingChange;
use super::window::{self, Action, Geometry};
use crate::tray::{self, TrayModel};

impl App {
    /// Carries out a window decision.
    pub(super) fn run_window_action(&mut self, action: Action) -> Task<Message> {
        match action {
            Action::None => Task::none(),
            Action::Open => {
                if self.window_id.is_some() {
                    return Task::none();
                }
                let (id, open) = iced::window::open(window::window_settings(self.geometry));
                self.window_id = Some(id);
                // Images were dropped while hidden: fetch what the screen needs.
                let images = self.request_images();
                Task::batch([open.map(|_| Message::Done(Done::Noop)), images])
            }
            Action::Close => {
                let Some(id) = self.window_id.take() else {
                    return Task::none();
                };
                self.geometry = Some(Geometry {
                    size: self.window_size,
                    position: self.window_pos,
                });
                self.menu = None;
                self.dialog = None;
                self.release_view_caches();
                iced::window::close(id)
            }
            Action::Raise => match self.window_id {
                Some(id) => Task::batch([
                    iced::window::minimize(id, false),
                    iced::window::gain_focus(id),
                ]),
                None => Task::none(),
            },
            Action::Exit => self.quit(),
        }
    }

    /// Drops state that only exists to speed up drawing (decoded cover art);
    /// it is fetched again from the HTTP cache when the window comes back.
    fn release_view_caches(&mut self) {
        self.images = super::images::ImageCache::new(super::images::DEFAULT_BUDGET);
        self.image_requests.clear();
    }

    pub(super) fn show_window(&mut self) -> Task<Message> {
        let a = self.win.show();
        self.run_window_action(a)
    }

    pub(super) fn on_tray(&mut self, ev: tray::Event) -> Task<Message> {
        match ev {
            tray::Event::Ready(control) => {
                self.win.tray_ready();
                self.tray_control = Some(control);
                Task::none()
            }
            tray::Event::Available => {
                self.win.tray_ready();
                Task::none()
            }
            tray::Event::Unavailable => {
                let a = self.win.tray_lost();
                self.run_window_action(a)
            }
            tray::Event::Command(c) => self.update_ui(window::command_to_ui(c)),
        }
    }

    /// What the tray should currently show.
    pub fn tray_model(&self) -> TrayModel {
        TrayModel {
            visible: self.win.visible,
            track: self
                .now_track()
                .map(|t| format!("{} \u{2014} {}", t.artist_line(), t.name)),
            playing: self.now_playback().is_playing(),
            start_minimized: self.settings.start_minimized,
        }
    }

    /// Pushes the current state to the tray (a no-op when unchanged).
    pub(super) fn sync_tray(&self) {
        if let Some(control) = &self.tray_control {
            control.publish(self.tray_model());
        }
    }

    pub(super) fn set_autostart(&mut self, want: bool) -> Task<Message> {
        if self.is_mock() {
            // Mock mode never writes to disk.
            self.autostart = want;
            return Task::none();
        }
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || tray::autostart::set_enabled(want))
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()))
            },
            |r| Message::Done(Done::AutostartChanged(r)),
        )
    }

    /// Window, tray and settings messages.
    pub(super) fn update_background(&mut self, u: Ui) -> Task<Message> {
        match u {
            Ui::CloseRequested => {
                let a = self.win.close_requested();
                self.run_window_action(a)
            }
            Ui::ToggleWindow => {
                let a = self.win.toggle();
                self.run_window_action(a)
            }
            Ui::ShowWindow => self.show_window(),
            Ui::HideWindow => {
                let a = self.win.hide();
                self.run_window_action(a)
            }
            Ui::ToggleStartMinimized => self.change_setting(SettingChange::StartMinimized(
                !self.settings.start_minimized,
            )),
            _ => Task::none(),
        }
    }
}
