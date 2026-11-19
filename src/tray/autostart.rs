//! XDG autostart entry (`~/.config/autostart/dev.ferro.ferrotune.desktop`)
//! written by the "Open Ferrotune at login" toggle. The file's existence is
//! the source of truth for the toggle; nothing is stored elsewhere.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

/// Desktop entry file name (matches the Wayland `app_id`).
pub const FILE_NAME: &str = "dev.ferro.ferrotune.desktop";

/// Whether the platform supports the toggle (XDG autostart is Linux only;
/// Windows / macOS are not implemented).
pub const SUPPORTED: bool = cfg!(target_os = "linux");

/// `$XDG_CONFIG_HOME/autostart`, or `$HOME/.config/autostart`. Pure so it
/// can be tested with a fake environment.
pub fn dir_from_env(xdg_config_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let base = xdg_config_home
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            home.map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|h| h.join(".config"))
        })?;
    Some(base.join("autostart"))
}

/// The autostart directory of the current user.
pub fn default_dir() -> Option<PathBuf> {
    dir_from_env(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

/// Quotes one `Exec` argument per the Desktop Entry spec: double quotes,
/// with `"`, `` ` ``, `$` and `\` backslash-escaped, then every backslash
/// doubled for the key/value syntax; `%` becomes `%%`.
fn quote_exec_arg(arg: &str) -> String {
    let mut quoted = String::from("\"");
    for c in arg.chars() {
        match c {
            '"' | '`' | '$' => {
                quoted.push_str("\\\\");
                quoted.push(c);
            }
            '\\' => quoted.push_str("\\\\\\\\"),
            '%' => quoted.push_str("%%"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// Contents of the desktop entry that launches `exe` minimized to the tray.
pub fn entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={name}\n\
         Comment=Lightweight Spotify client (unofficial)\n\
         Exec={exec} --minimized\n\
         Icon={id}\n\
         Terminal=false\n\
         Categories=AudioVideo;Audio;Player;\n\
         StartupWMClass={id}\n\
         X-GNOME-Autostart-enabled=true\n",
        name = crate::config::APP_NAME,
        exec = quote_exec_arg(&exe.to_string_lossy()),
        id = crate::config::APP_ID,
    )
}

pub fn is_enabled(dir: &Path) -> bool {
    dir.join(FILE_NAME).is_file()
}

/// Writes the entry (creating the directory).
pub fn enable(dir: &Path, exe: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(FILE_NAME), entry(exe))
}

/// Removes the entry; already absent is fine.
pub fn disable(dir: &Path) -> io::Result<()> {
    match std::fs::remove_file(dir.join(FILE_NAME)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Applies the toggle for the current user and executable. Blocking.
pub fn set_enabled(on: bool) -> Result<bool, String> {
    if !SUPPORTED {
        return Err("Starting at login is not supported on this platform".into());
    }
    let dir = default_dir().ok_or("cannot locate the autostart directory")?;
    let result = if on {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        enable(&dir, &exe)
    } else {
        disable(&dir)
    };
    result.map_err(|e| e.to_string())?;
    Ok(is_enabled(&dir))
}

/// Current state for the current user. Blocking (one `stat`).
pub fn current() -> bool {
    SUPPORTED && default_dir().is_some_and(|d| is_enabled(&d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_prefers_xdg_config_home() {
        assert_eq!(
            dir_from_env(Some("/x/cfg".into()), Some("/home/u".into())),
            Some(PathBuf::from("/x/cfg/autostart"))
        );
        assert_eq!(
            dir_from_env(None, Some("/home/u".into())),
            Some(PathBuf::from("/home/u/.config/autostart"))
        );
        // Relative XDG paths are ignored, per the basedir spec.
        assert_eq!(
            dir_from_env(Some("rel".into()), Some("/home/u".into())),
            Some(PathBuf::from("/home/u/.config/autostart"))
        );
        assert_eq!(dir_from_env(None, None), None);
    }

    #[test]
    fn entry_launches_minimized() {
        let e = entry(Path::new("/opt/Ferro tune/ferrotune"));
        assert!(e.starts_with("[Desktop Entry]\n"));
        assert!(e.contains("Exec=\"/opt/Ferro tune/ferrotune\" --minimized\n"));
        assert!(e.contains("Type=Application\n"));
        assert!(e.contains("Icon=dev.ferro.ferrotune\n"));
    }

    #[test]
    fn exec_quoting_escapes_specials() {
        assert_eq!(quote_exec_arg("a$b"), "\"a\\\\$b\"");
        assert_eq!(quote_exec_arg("50%"), "\"50%%\"");
        assert_eq!(quote_exec_arg("a\\b"), "\"a\\\\\\\\b\"");
    }

    #[test]
    fn enable_and_disable_in_a_temp_config_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = dir_from_env(Some(tmp.path().into()), None).unwrap();
        assert!(!is_enabled(&dir));
        enable(&dir, Path::new("/usr/bin/ferrotune")).unwrap();
        assert!(is_enabled(&dir));
        let text = std::fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(text.contains("--minimized"));
        // Idempotent.
        enable(&dir, Path::new("/usr/bin/ferrotune")).unwrap();
        disable(&dir).unwrap();
        assert!(!is_enabled(&dir));
        disable(&dir).unwrap();
    }
}
