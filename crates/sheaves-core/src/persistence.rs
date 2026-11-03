//! JSON files written whole, so what is on disk is never half updated.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Where Sheaves keeps its files: Application Support on a Mac (inside the app's
/// sandbox container when sandboxed), `~/.local/share/sheaves` on Linux.
pub fn data_directory() -> PathBuf {
    let base = directories::BaseDirs::new()
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir);
    let name = if cfg!(target_os = "macos") {
        "Sheaves"
    } else {
        "sheaves"
    };
    base.join(name)
}

/// The value stored at `path`, or `None` when there is no file or it cannot be read
/// as a `T`. A missing or outdated file is not an error to the caller: it starts
/// empty and waits for the network.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let data = std::fs::read(path).ok()?;
    match serde_json::from_slice(&data) {
        Ok(value) => Some(value),
        Err(error) => {
            log::error!("could not read {}: {error}", path.display());
            None
        }
    }
}

/// Writes `value` to `path` through a temporary file and a rename, so a crash
/// mid-write leaves the previous file intact.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let directory = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(directory)?;
    let data = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.write_all(&data)?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

pub fn remove(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => log::error!("could not remove {}: {error}", path.display()),
    }
}
