//! Starting at login: a desktop entry in `~/.config/autostart`, which GNOME runs
//! when the session starts.
//!
//! The entry runs the binary where it is now. There is no package to install, so
//! moving the checkout needs the switch turned off and on.

use std::io;
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "com.rainhead.Sheaves.desktop";

/// The autostart folder: `$XDG_CONFIG_HOME/autostart`, or `~/.config/autostart`.
pub fn folder() -> PathBuf {
    gtk::glib::user_config_dir().join("autostart")
}

pub fn is_enabled(folder: &Path) -> bool {
    folder.join(FILE_NAME).exists()
}

/// The program to start at login: this one, as it is on disk now.
///
/// Linux names a running program whose file has since been replaced, as `cargo
/// build` replaces it, with " (deleted)" after its path. The rebuilt file at that
/// path is the one to run, so the suffix is dropped, and a path with no file at it
/// is refused rather than written into an entry that would start nothing.
pub fn this_program() -> io::Result<PathBuf> {
    let running = std::env::current_exe()?;
    let path = running
        .to_str()
        .and_then(|text| text.strip_suffix(" (deleted)"))
        .map_or(running.clone(), PathBuf::from);
    if path.is_file() {
        Ok(path)
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} is no longer there", path.display()),
        ))
    }
}

/// Writes the entry, running `program`.
pub fn enable(folder: &Path, program: &Path) -> io::Result<()> {
    std::fs::create_dir_all(folder)?;
    std::fs::write(folder.join(FILE_NAME), entry(program))
}

/// Removes the entry. Removing one that is not there is not an error.
pub fn disable(folder: &Path) -> io::Result<()> {
    match std::fs::remove_file(folder.join(FILE_NAME)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn entry(program: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Sheaves\n\
         Comment=Harvest timer in the top bar\n\
         Exec={} --background\n\
         Icon=alarm-symbolic\n\
         X-GNOME-Autostart-enabled=true\n\
         NoDisplay=true\n",
        quoted(program)
    )
}

/// `path` as one argument of an `Exec` line: in double quotes, with the
/// characters the Desktop Entry spec reserves inside them escaped.
///
/// Two layers of escaping apply. The `Exec` value is a string in a key file,
/// whose own escapes are undone first, and inside its quotes `"`, `` ` ``, `$`
/// and `\` each need a backslash before them. So `"` is written `\\"`, and a
/// backslash, escaped once for the quotes and again for the key file, `\\\\`.
fn quoted(path: &Path) -> String {
    let mut quoted = String::from("\"");
    for character in path.to_string_lossy().chars() {
        match character {
            '"' | '`' | '$' => {
                quoted.push_str("\\\\");
                quoted.push(character);
            }
            '\\' => quoted.push_str("\\\\\\\\"),
            '%' => quoted.push_str("%%"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enables_and_disables() {
        let folder = tempfile::tempdir().unwrap();
        let autostart = folder.path().join("autostart");
        assert!(!is_enabled(&autostart));
        enable(
            &autostart,
            Path::new("/home/me/dev/sheaves/target/release/sheaves-gnome"),
        )
        .unwrap();
        assert!(is_enabled(&autostart));
        let written = std::fs::read_to_string(autostart.join(FILE_NAME)).unwrap();
        assert!(
            written.contains(
                "Exec=\"/home/me/dev/sheaves/target/release/sheaves-gnome\" --background\n"
            )
        );
        disable(&autostart).unwrap();
        assert!(!is_enabled(&autostart));
        disable(&autostart).unwrap();
    }

    /// The `Exec` line has its own quoting: a space would split the path in two,
    /// and a `%` would be read as a field code.
    #[test]
    fn quotes_the_program() {
        assert_eq!(quoted(Path::new("/a b/100%/x")), "\"/a b/100%%/x\"");
        assert_eq!(quoted(Path::new("/a\"b")), "\"/a\\\\\"b\"");
        assert_eq!(quoted(Path::new("/a$b`c")), "\"/a\\\\$b\\\\`c\"");
        assert_eq!(quoted(Path::new("/a\\b")), "\"/a\\\\\\\\b\"");
    }
}
