//! How GNOME knows Sheaves by name and icon: a desktop entry and an app icon,
//! both named after the app id, in the user's own data folder.
//!
//! GNOME matches a window to its app by the app id, so a window with no desktop
//! entry behind it shows a generic gear in the dock and the window switcher. There
//! is no package to install one, so the app installs its own each time it starts,
//! pointing at the binary where it is. That also puts Sheaves in the app grid.

use std::io;
use std::path::Path;

use crate::app::APP_ID;

const APP_ICON: &str = include_str!("../../../icons/com.rainhead.Sheaves.svg");

/// Installs the entry and the icon under `data` (normally `~/.local/share`), for
/// `program`. Files already as they should be are left alone.
pub fn install(data: &Path, program: &Path) -> io::Result<()> {
    write_if_changed(
        &data.join(format!("icons/hicolor/scalable/apps/{APP_ID}.svg")),
        APP_ICON,
    )?;
    write_if_changed(
        &data.join(format!("applications/{APP_ID}.desktop")),
        &entry(program),
    )
}

fn entry(program: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Sheaves\n\
         Comment=Track time in Harvest from the top bar\n\
         Exec={}\n\
         Icon={APP_ID}\n\
         Categories=Office;ProjectManagement;\n\
         StartupNotify=true\n",
        crate::autostart::quoted(program)
    )
}

fn write_if_changed(path: &Path, contents: &str) -> io::Result<()> {
    if std::fs::read_to_string(path).ok().as_deref() == Some(contents) {
        return Ok(());
    }
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    std::fs::write(path, contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_an_entry_named_for_the_app_and_its_icon() {
        let data = tempfile::tempdir().unwrap();
        let program = Path::new("/home/me/dev/sheaves/target/release/sheaves-gnome");
        install(data.path(), program).unwrap();

        let entry = std::fs::read_to_string(
            data.path()
                .join("applications/com.rainhead.Sheaves.desktop"),
        )
        .unwrap();
        assert!(entry.contains("Exec=\"/home/me/dev/sheaves/target/release/sheaves-gnome\"\n"));
        assert!(entry.contains("Icon=com.rainhead.Sheaves\n"));
        let icon = data
            .path()
            .join("icons/hicolor/scalable/apps/com.rainhead.Sheaves.svg");
        assert_eq!(std::fs::read_to_string(icon).unwrap(), APP_ICON);
    }

    /// A rebuild elsewhere moves the binary; the entry follows it on next launch.
    #[test]
    fn follows_the_program_when_it_moves() {
        let data = tempfile::tempdir().unwrap();
        install(data.path(), Path::new("/old/sheaves-gnome")).unwrap();
        install(data.path(), Path::new("/new/sheaves-gnome")).unwrap();
        let entry = std::fs::read_to_string(
            data.path()
                .join("applications/com.rainhead.Sheaves.desktop"),
        )
        .unwrap();
        assert!(entry.contains("Exec=\"/new/sheaves-gnome\"\n"));
    }
}
