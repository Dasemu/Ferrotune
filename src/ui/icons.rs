//! Lucide icons (ISC licence, see assets/icons/LICENSE), embedded at build time.

use iced::widget::svg;

macro_rules! icons {
    ($($name:ident => $file:literal),* $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Icon { $($name),* }

        impl Icon {
            pub fn bytes(self) -> &'static [u8] {
                match self {
                    $(Self::$name => include_bytes!(concat!("../../assets/icons/", $file, ".svg"))),*
                }
            }
        }
    };
}

icons! {
    Home => "house",
    Search => "search",
    Library => "library-big",
    Plus => "plus",
    Heart => "heart",
    HeartFilled => "heart-filled",
    Play => "play",
    Pause => "pause",
    SkipBack => "skip-back",
    SkipForward => "skip-forward",
    Shuffle => "shuffle",
    Repeat => "repeat",
    RepeatOne => "repeat-1",
    Volume => "volume-2",
    VolumeLow => "volume-1",
    VolumeMute => "volume-x",
    Queue => "list-music",
    ChevronLeft => "chevron-left",
    ChevronRight => "chevron-right",
    Clock => "clock-3",
    Music => "music",
    User => "user",
    Disc => "disc-3",
    Close => "x",
    LogOut => "log-out",
    More => "ellipsis",
    ArrowRight => "arrow-right",
    List => "list",
    Alert => "circle-alert",
    Loader => "loader-circle",
    Maximize => "maximize-2",
    Device => "laptop-minimal",
    Lyrics => "mic-vocal",
    Smartphone => "smartphone",
    Speaker => "speaker",
    Tv => "tv",
    Settings => "settings",
}

pub fn handle(icon: Icon) -> svg::Handle {
    svg::Handle::from_memory(icon.bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_are_svg() {
        for i in [Icon::Home, Icon::Play, Icon::HeartFilled, Icon::Lyrics] {
            assert!(std::str::from_utf8(i.bytes()).unwrap().contains("<svg"));
        }
    }
}
