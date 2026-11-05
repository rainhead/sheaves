//! The key that opens quick entry: one of GNOME's custom keyboard shortcuts,
//! running `sheaves-gnome --quick-entry`.
//!
//! GNOME's settings daemon owns global shortcuts, so a key that works from any
//! app is one listed there, under Settings › Keyboard › Custom Shortcuts. Sheaves
//! adds its own when asked, at a path of its own, and leaves the rest alone. The
//! GlobalShortcuts portal is the other route, if a binding inside the app is ever
//! wanted; it needs the app to be running when the key is bound, which this does
//! not.

use std::path::Path;

use gtk::gio;
use gtk::prelude::*;

const LIST_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys";
const BINDING_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";
const PATH: &str =
    "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/sheaves-quick-entry/";

/// The key Sheaves suggests: the Mac's ⌃⌥⌘T, with Super for ⌘. Ctrl+Alt+T is
/// Ubuntu's terminal.
pub const SUGGESTED: &str = "<Control><Alt><Super>t";

/// What starting quick entry runs, with the path quoted for a shell. GNOME's
/// settings daemon doubles any `%` itself before it expands a desktop entry's
/// `%` codes, then reads the command as a shell would.
pub fn command(program: &Path) -> String {
    let quoted = gtk::glib::shell_quote(program);
    format!("{} --quick-entry", quoted.to_string_lossy())
}

/// Where the shortcuts are kept: the desktop's settings, or a stand-in in tests.
pub struct Shortcuts {
    backend: Option<gio::SettingsBackend>,
}

impl Shortcuts {
    /// The desktop's own, or `None` where GNOME's settings daemon is not
    /// installed and there is nowhere to put a shortcut.
    pub fn desktop() -> Option<Self> {
        let source = gio::SettingsSchemaSource::default()?;
        (source.lookup(LIST_SCHEMA, true).is_some()
            && source.lookup(BINDING_SCHEMA, true).is_some())
        .then_some(Self { backend: None })
    }

    #[cfg(test)]
    fn in_memory() -> Option<Self> {
        Self::desktop()?;
        Some(Self {
            backend: Some(gio::memory_settings_backend_new()),
        })
    }

    fn settings(&self, schema: &str, path: Option<&str>) -> gio::Settings {
        let source = gio::SettingsSchemaSource::default().expect("checked in desktop()");
        let schema = source.lookup(schema, true).expect("checked in desktop()");
        gio::Settings::new_full(&schema, self.backend.as_ref(), path)
    }

    fn list(&self) -> gio::Settings {
        self.settings(LIST_SCHEMA, None)
    }

    fn binding(&self) -> gio::Settings {
        self.settings(BINDING_SCHEMA, Some(PATH))
    }

    /// The key bound to quick entry, as GNOME writes it (`<Control><Alt><Super>t`),
    /// or `None` when Sheaves' shortcut is not in the list.
    pub fn bound(&self) -> Option<String> {
        let listed = self.list().strv("custom-keybindings");
        if !listed.iter().any(|path| path.as_str() == PATH) {
            return None;
        }
        let key = self.binding().string("binding").to_string();
        (!key.is_empty()).then_some(key)
    }

    /// Adds Sheaves' shortcut with `key`, running `program`, or brings it up to
    /// date if it is there already: the binary may have moved since.
    pub fn bind(&self, key: &str, program: &Path) -> Result<(), gtk::glib::BoolError> {
        let binding = self.binding();
        binding.set_string("name", "Sheaves quick entry")?;
        binding.set_string("command", &command(program))?;
        if binding.string("binding").is_empty() || self.bound().is_none() {
            binding.set_string("binding", key)?;
        }
        let list = self.list();
        let mut listed: Vec<String> = list
            .strv("custom-keybindings")
            .iter()
            .map(|path| path.to_string())
            .collect();
        if !listed.iter().any(|path| path == PATH) {
            listed.push(PATH.to_string());
            let listed: Vec<&str> = listed.iter().map(String::as_str).collect();
            list.set_strv("custom-keybindings", listed.as_slice())?;
        }
        Ok(())
    }

    /// Keeps the shortcut's command pointing at `program`, if there is one.
    pub fn refresh(&self, program: &Path) {
        if self.bound().is_some()
            && self.binding().string("command") != command(program)
            && let Err(error) = self.binding().set_string("command", &command(program))
        {
            log::warn!("could not point the quick entry shortcut at {program:?}: {error}");
        }
    }

    /// Takes Sheaves' shortcut out of the list, leaving any others.
    pub fn unbind(&self) -> Result<(), gtk::glib::BoolError> {
        let list = self.list();
        let kept: Vec<String> = list
            .strv("custom-keybindings")
            .iter()
            .map(|path| path.to_string())
            .filter(|path| path != PATH)
            .collect();
        let kept: Vec<&str> = kept.iter().map(String::as_str).collect();
        list.set_strv("custom-keybindings", kept.as_slice())?;
        self.binding().reset("binding");
        self.binding().reset("command");
        self.binding().reset("name");
        Ok(())
    }
}

/// `<Control><Alt><Super>t` as a person reads it: "Ctrl+Alt+Super+T".
pub fn readable(key: &str) -> String {
    gtk::accelerator_parse(key)
        .map(|(key, modifiers)| gtk::accelerator_get_label(key, modifiers).to_string())
        .unwrap_or_else(|| key.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const OTHER: &str = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/custom0/";

    /// Skipped where GNOME's settings daemon is not installed, as on a Mac.
    fn shortcuts() -> Option<Shortcuts> {
        let shortcuts = Shortcuts::in_memory();
        if shortcuts.is_none() {
            eprintln!("skipped: no GNOME settings schemas here");
        }
        shortcuts
    }

    fn listed(shortcuts: &Shortcuts) -> Vec<String> {
        shortcuts
            .list()
            .strv("custom-keybindings")
            .iter()
            .map(|path| path.to_string())
            .collect()
    }

    #[test]
    fn adds_its_own_and_keeps_the_others() {
        let Some(shortcuts) = shortcuts() else { return };
        shortcuts
            .list()
            .set_strv("custom-keybindings", [OTHER].as_slice())
            .unwrap();
        assert_eq!(shortcuts.bound(), None);

        let program = Path::new("/home/me/dev/sheaves/target/debug/sheaves-gnome");
        shortcuts.bind(SUGGESTED, program).unwrap();

        assert_eq!(shortcuts.bound().as_deref(), Some(SUGGESTED));
        assert_eq!(listed(&shortcuts), [OTHER, PATH]);
        assert_eq!(shortcuts.binding().string("command"), command(program));

        // Bound twice, it is listed once.
        shortcuts.bind(SUGGESTED, program).unwrap();
        assert_eq!(listed(&shortcuts).len(), 2);

        shortcuts.unbind().unwrap();
        assert_eq!(shortcuts.bound(), None);
        assert_eq!(listed(&shortcuts), [OTHER]);
    }

    /// A key changed in GNOME's Settings is kept when Sheaves updates the command.
    #[test]
    fn keeps_a_key_the_user_changed() {
        let Some(shortcuts) = shortcuts() else { return };
        let program = Path::new("/opt/sheaves-gnome");
        shortcuts.bind(SUGGESTED, program).unwrap();
        shortcuts
            .binding()
            .set_string("binding", "<Super>space")
            .unwrap();

        shortcuts
            .bind(SUGGESTED, Path::new("/elsewhere/sheaves-gnome"))
            .unwrap();
        assert_eq!(shortcuts.bound().as_deref(), Some("<Super>space"));

        shortcuts.refresh(Path::new("/moved/sheaves-gnome"));
        assert_eq!(
            shortcuts.binding().string("command"),
            command(Path::new("/moved/sheaves-gnome"))
        );
    }

    /// Read back as GNOME reads it: the program and its one flag, however odd
    /// the folder's name.
    #[test]
    fn runs_the_program_with_quick_entry() {
        for program in [
            "/home/me/my apps/sheaves-gnome",
            "/home/me/it's 100%/sheaves-gnome",
        ] {
            let line = command(Path::new(program));
            let argv = gtk::glib::shell_parse_argv(&line).unwrap();
            assert_eq!(
                argv,
                [std::ffi::OsString::from(program), "--quick-entry".into()]
            );
        }
    }
}
