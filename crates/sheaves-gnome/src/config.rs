//! The app's own settings, kept as JSON in `~/.config/sheaves/settings.json`.
//!
//! GSettings would need a schema installed, and there is no package to install
//! one, so a file it is. Only what is not the core's: the core keeps its own state.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// The other machines to share presence with, by their names on the tailnet.
    #[serde(default)]
    pub peers: Vec<String>,
}

/// Where the settings are kept.
pub fn path() -> PathBuf {
    gtk::glib::user_config_dir()
        .join("sheaves")
        .join("settings.json")
}

impl Config {
    /// The settings at `path`, or the defaults when there are none or they cannot
    /// be read; a file that cannot be read is logged, not lost silently.
    pub fn read(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(data) => serde_json::from_slice(&data).unwrap_or_else(|error| {
                log::warn!(
                    "ignoring unreadable settings in {}: {error}",
                    path.display()
                );
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let data = serde_json::to_vec_pretty(self).expect("settings encode");
        // Whole or not at all, so a crash cannot leave half a file.
        let temporary = path.with_extension("json.new");
        std::fs::write(&temporary, data)?;
        std::fs::rename(&temporary, path)
    }
}

/// The names in what was typed, separated by commas, spaces or new lines, once
/// each, as the Mac's Settings reads them.
pub fn names_in(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in text.split(|c: char| c == ',' || c.is_whitespace()) {
        if !name.is_empty() && !names.iter().any(|known| known == name) {
            names.push(name.to_string());
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_names_as_the_mac_does() {
        assert_eq!(
            names_in("peters-macbook-air, hostess\npeters-macbook-air  "),
            ["peters-macbook-air", "hostess"]
        );
        assert!(names_in(" , ").is_empty());
    }

    #[test]
    fn keeps_the_peers() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("sheaves/settings.json");
        assert_eq!(Config::read(&path), Config::default());
        let config = Config {
            peers: vec!["peters-macbook-air".into()],
        };
        config.write(&path).unwrap();
        assert_eq!(Config::read(&path), config);
    }

    #[test]
    fn an_unreadable_file_reads_as_the_defaults() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("settings.json");
        std::fs::write(&path, b"{").unwrap();
        assert_eq!(Config::read(&path), Config::default());
    }
}
