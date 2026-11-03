//! Sheaves for GNOME: the running timer in the top bar, and the day in a window.
//!
//! Everything but the views is the shared core (`crates/sheaves-core`), the same
//! one the Mac app runs on. `--background` starts in the top bar with no window,
//! which is how the login item starts it.

mod app;
mod autostart;
#[cfg(debug_assertions)]
mod documentation;
mod format;
mod indicator;
mod keyring;
mod settings;
mod window;

fn main() -> gtk::glib::ExitCode {
    install_logger();
    #[cfg(debug_assertions)]
    if let Some(directory) = std::env::var_os("SHEAVES_RENDER") {
        return documentation::render(directory.into());
    }
    app::run()
}

/// Sends the core's log lines and the app's own to standard error, which the
/// session's journal keeps for an app started at login:
/// `journalctl --user -t sheaves-gnome`.
fn install_logger() {
    struct Stderr;

    impl log::Log for Stderr {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.level() <= log::Level::Info
        }

        fn log(&self, record: &log::Record<'_>) {
            if self.enabled(record.metadata()) {
                eprintln!("{} {}: {}", record.level(), record.target(), record.args());
            }
        }

        fn flush(&self) {}
    }

    if log::set_boxed_logger(Box::new(Stderr)).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}
