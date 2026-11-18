#![deny(clippy::unwrap_used, clippy::expect_used)]
//! Entry point: logging, config, `iced::application(..).run()`.
//!
//! Usage: `ferrotune [--mock] [--screenshot <screen> <out.png>]`
//! Screens: login, home, search, library, playlist, album, artist, liked, queue.

use std::time::Instant;

use ferrotune::app::{App, Options, ScreenshotPlan};
use ferrotune::config::{APP_ID, Config};
use ferrotune::ui::{self, theme};

fn usage() -> ! {
    eprintln!("usage: ferrotune [--mock] [--screenshot <screen> <out.png>]");
    std::process::exit(2);
}

fn main() -> iced::Result {
    let started = Instant::now();
    let mut mock = false;
    let mut screenshot = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--mock" => mock = true,
            "--screenshot" => {
                let (Some(screen), Some(out)) = (args.next(), args.next()) else {
                    usage()
                };
                screenshot = Some(ScreenshotPlan {
                    screen,
                    out: out.into(),
                });
                mock = true;
            }
            "-h" | "--help" => usage(),
            _ => usage(),
        }
    }
    let config = Config::load(mock);
    ferrotune::logging::init((!mock).then(|| config.log_path()).as_deref());
    let ferro = theme::FerroTheme::new(config.theme);
    theme::set_current(ferro);
    let iced_theme = ferro.to_iced();
    let opts = Options {
        screenshot,
        started: Some(started),
    };

    iced::application(
        move || App::boot(config.clone(), opts.clone()),
        App::update,
        ui::view,
    )
    .title(App::title)
    .subscription(App::subscription)
    .theme(move |_: &App| iced_theme.clone())
    .font(theme::INTER_REGULAR)
    .font(theme::INTER_SEMIBOLD)
    .font(theme::INTER_BOLD)
    .default_font(theme::FONT)
    .window(iced::window::Settings {
        size: iced::Size::new(1280.0, 800.0),
        min_size: Some(iced::Size::new(800.0, 600.0)),
        platform_specific: iced::window::settings::PlatformSpecific {
            #[cfg(target_os = "linux")]
            application_id: APP_ID.to_owned(),
            ..Default::default()
        },
        ..Default::default()
    })
    .run()
}
