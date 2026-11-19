//! Keyboard shortcuts, mirroring the Spotify desktop client.
//!
//! `map` is a pure function from a key press to a UI message so the whole
//! table is unit-tested without iced. On macOS the `Ctrl` shortcuts use
//! `Cmd`, via `Modifiers::command()`.

use iced::keyboard::key::Named;
use iced::keyboard::{Key, Modifiers};

use super::message::Ui;

/// Volume change per Ctrl+Up / Ctrl+Down press.
pub const VOLUME_STEP: f32 = 0.1;

/// Rows of the shortcuts dialog: `(section, [(keys, what it does)])`.
/// Kept next to `map` so the two cannot drift apart (a test checks this).
pub const HELP: &[(&str, &[(&str, &str)])] = &[
    (
        "Playback",
        &[
            ("Space", "Play / pause"),
            ("Ctrl + Right", "Next track"),
            ("Ctrl + Left", "Previous track"),
            ("Ctrl + Up", "Volume up"),
            ("Ctrl + Down", "Volume down"),
            ("Ctrl + S", "Toggle shuffle"),
            ("Ctrl + R", "Cycle repeat"),
        ],
    ),
    (
        "Navigation",
        &[
            ("Ctrl + L  or  Ctrl + K", "Search"),
            ("Alt + Left", "Go back"),
            ("Alt + Right", "Go forward"),
            ("Esc", "Close menu or dialog"),
        ],
    ),
    (
        "General",
        &[
            ("Ctrl + /  or  ?", "Show keyboard shortcuts"),
            ("Ctrl + Q", "Quit"),
        ],
    ),
];

/// The UI message for a key press, if it is a shortcut.
pub fn map(key: &Key, mods: Modifiers) -> Option<Ui> {
    let ctrl = mods.command() && !mods.alt();
    let alt = mods.alt() && !mods.command();
    let plain = !mods.command() && !mods.alt();
    match key {
        Key::Named(Named::Space) if plain && !mods.shift() => Some(Ui::TogglePlay),
        Key::Named(Named::Escape) => Some(Ui::CloseOverlay),
        Key::Named(Named::ArrowRight) if ctrl => Some(Ui::Next),
        Key::Named(Named::ArrowLeft) if ctrl => Some(Ui::Previous),
        Key::Named(Named::ArrowUp) if ctrl => Some(Ui::VolumeStep(VOLUME_STEP)),
        Key::Named(Named::ArrowDown) if ctrl => Some(Ui::VolumeStep(-VOLUME_STEP)),
        Key::Named(Named::ArrowLeft) if alt => Some(Ui::Back),
        Key::Named(Named::ArrowRight) if alt => Some(Ui::Forward),
        Key::Character(c) => {
            let c = c.to_lowercase();
            match c.as_str() {
                "l" | "k" if ctrl && !mods.shift() => Some(Ui::FocusSearch),
                "s" if ctrl && !mods.shift() => Some(Ui::ToggleShuffle),
                "r" if ctrl && !mods.shift() => Some(Ui::CycleRepeat),
                "q" if ctrl && !mods.shift() => Some(Ui::Quit),
                "/" if ctrl => Some(Ui::ToggleHelp),
                "?" if plain => Some(Ui::ToggleHelp),
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(s: &str) -> Key {
        Key::Character(s.into())
    }

    fn name(n: Named) -> Key {
        Key::Named(n)
    }

    const NONE: Modifiers = Modifiers::empty();
    const CTRL: Modifiers = Modifiers::CTRL;
    const ALT: Modifiers = Modifiers::ALT;

    #[test]
    fn space_toggles_play_only_without_modifiers() {
        assert!(matches!(
            map(&name(Named::Space), NONE),
            Some(Ui::TogglePlay)
        ));
        assert!(map(&name(Named::Space), CTRL).is_none());
        assert!(map(&name(Named::Space), Modifiers::SHIFT).is_none());
    }

    #[test]
    fn transport_shortcuts() {
        assert!(matches!(
            map(&name(Named::ArrowRight), CTRL),
            Some(Ui::Next)
        ));
        assert!(matches!(
            map(&name(Named::ArrowLeft), CTRL),
            Some(Ui::Previous)
        ));
        assert!(matches!(map(&ch("s"), CTRL), Some(Ui::ToggleShuffle)));
        assert!(matches!(map(&ch("r"), CTRL), Some(Ui::CycleRepeat)));
        assert!(matches!(map(&ch("R"), CTRL), Some(Ui::CycleRepeat)));
    }

    #[test]
    fn volume_steps_have_the_right_sign() {
        match map(&name(Named::ArrowUp), CTRL) {
            Some(Ui::VolumeStep(d)) => assert!(d > 0.0),
            other => panic!("{other:?}"),
        }
        match map(&name(Named::ArrowDown), CTRL) {
            Some(Ui::VolumeStep(d)) => assert!(d < 0.0),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn search_back_forward_quit() {
        assert!(matches!(map(&ch("l"), CTRL), Some(Ui::FocusSearch)));
        assert!(matches!(map(&ch("k"), CTRL), Some(Ui::FocusSearch)));
        assert!(matches!(map(&name(Named::ArrowLeft), ALT), Some(Ui::Back)));
        assert!(matches!(
            map(&name(Named::ArrowRight), ALT),
            Some(Ui::Forward)
        ));
        assert!(matches!(map(&ch("q"), CTRL), Some(Ui::Quit)));
    }

    #[test]
    fn help_and_escape() {
        assert!(matches!(map(&ch("/"), CTRL), Some(Ui::ToggleHelp)));
        assert!(matches!(
            map(&ch("?"), Modifiers::SHIFT),
            Some(Ui::ToggleHelp)
        ));
        assert!(matches!(
            map(&name(Named::Escape), NONE),
            Some(Ui::CloseOverlay)
        ));
    }

    #[test]
    fn unrelated_keys_do_nothing() {
        assert!(map(&ch("a"), NONE).is_none());
        assert!(map(&ch("l"), NONE).is_none());
        assert!(map(&ch("s"), ALT).is_none());
        assert!(map(&name(Named::ArrowRight), NONE).is_none());
        // Ctrl+Alt combinations are left to the desktop environment.
        assert!(map(&name(Named::ArrowLeft), CTRL | ALT).is_none());
    }

    #[test]
    fn every_help_row_is_nonempty() {
        for (_, rows) in HELP {
            assert!(!rows.is_empty());
            for (k, d) in *rows {
                assert!(!k.is_empty() && !d.is_empty());
            }
        }
    }
}
