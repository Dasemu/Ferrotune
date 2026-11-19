//! Desktop integration files: the `.desktop` entry (id matches the Wayland
//! `app_id`, so KDE/GNOME group windows and MPRIS under it) and the original
//! app icon. `ferrotune --install-desktop-entry` writes both for users who
//! installed with `cargo install` and have no package to do it.

use std::path::{Path, PathBuf};

use crate::config::APP_ID;
use crate::error::Result;

pub const DESKTOP_ENTRY: &str = include_str!("../assets/desktop/dev.ferro.ferrotune.desktop");
pub const ICON_SVG: &str = include_str!("../assets/desktop/dev.ferro.ferrotune.svg");

/// Where the files go under an XDG data dir (usually `~/.local/share`).
pub fn targets(data_home: &Path) -> (PathBuf, PathBuf) {
    (
        data_home
            .join("applications")
            .join(format!("{APP_ID}.desktop")),
        data_home
            .join("icons/hicolor/scalable/apps")
            .join(format!("{APP_ID}.svg")),
    )
}

/// Characters that force quoting in an `Exec` argument (Desktop Entry
/// Specification, "The Exec key").
const RESERVED: &[char] = &[
    ' ', '\t', '\n', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')',
    '`',
];

/// Quote one `Exec` argument: `%` is always doubled (field codes); with any
/// reserved character the argument is double-quoted and `"`, `` ` ``, `$`
/// and `\` are backslash-escaped. Finally every `\` is doubled again
/// because the value is also a desktop-entry *string* (escape level two).
pub fn exec_arg(arg: &str) -> String {
    let arg = arg.replace('%', "%%");
    if !arg.contains(RESERVED) {
        return arg;
    }
    let mut quoted = String::from("\"");
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted.replace('\\', "\\\\")
}

/// The entry with `Exec=` pointing at `exe`, so it works even when
/// `~/.cargo/bin` is not on the session PATH.
pub fn entry_for(exe: &Path) -> String {
    let exec = exec_arg(&exe.to_string_lossy());
    DESKTOP_ENTRY.replace("Exec=ferrotune\n", &format!("Exec={exec}\n"))
}

/// `$XDG_DATA_HOME`, else `~/.local/share`.
pub fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| directories::BaseDirs::new().map(|b| b.home_dir().join(".local/share")))
}

/// Write the entry and the icon; returns the two paths.
pub fn install(data_home: &Path, exe: &Path) -> Result<(PathBuf, PathBuf)> {
    let (entry, icon) = targets(data_home);
    for p in [&entry, &icon] {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
    }
    std::fs::write(&entry, entry_for(exe))?;
    std::fs::write(&icon, ICON_SVG)?;
    Ok((entry, icon))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_matches_app_id_and_spec_basics() {
        assert!(DESKTOP_ENTRY.starts_with("[Desktop Entry]"));
        assert!(DESKTOP_ENTRY.contains(&format!("Icon={APP_ID}\n")));
        assert!(DESKTOP_ENTRY.contains(&format!("StartupWMClass={APP_ID}\n")));
        assert!(ICON_SVG.contains("<svg"));
    }

    #[test]
    fn exec_is_absolute_and_quoted_when_needed() {
        let e = entry_for(Path::new("/home/a/.cargo/bin/ferrotune"));
        assert!(e.contains("Exec=/home/a/.cargo/bin/ferrotune\n"));
        let e = entry_for(Path::new("/opt/My Apps/ferrotune"));
        assert!(e.contains("Exec=\"/opt/My Apps/ferrotune\"\n"), "{e}");
    }

    #[test]
    fn exec_arg_escaping() {
        assert_eq!(exec_arg("/usr/bin/ferrotune"), "/usr/bin/ferrotune");
        // Field codes are escaped even without quoting.
        assert_eq!(exec_arg("/opt/100%/ft"), "/opt/100%%/ft");
        // Every reserved character forces quoting.
        for c in ["~", "&", ";", "(", ")", "*", "?", "#", "|", "<", ">", "'"] {
            let a = exec_arg(&format!("/a{c}b"));
            assert!(a.starts_with('"') && a.ends_with('"'), "{c}: {a}");
        }
        // Inside quotes `$`, `"` and `` ` `` get a backslash, and backslashes
        // are doubled once more for the string level.
        assert_eq!(exec_arg("/a b$c"), "\"/a b\\\\$c\"");
        assert_eq!(exec_arg("/a\\b"), "\"/a\\\\\\\\b\"");
    }

    #[test]
    fn install_writes_both_files() {
        let dir = std::env::temp_dir().join(format!("ferrotune-desktop-{}", std::process::id()));
        let (entry, icon) = install(&dir, Path::new("/usr/bin/ferrotune")).unwrap();
        assert!(entry.ends_with("applications/dev.ferro.ferrotune.desktop"));
        assert!(icon.ends_with("icons/hicolor/scalable/apps/dev.ferro.ferrotune.svg"));
        assert!(
            std::fs::read_to_string(&entry)
                .unwrap()
                .contains("Exec=/usr/bin/ferrotune")
        );
        // Idempotent.
        install(&dir, Path::new("/usr/bin/ferrotune")).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }
}
