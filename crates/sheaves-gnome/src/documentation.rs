//! Renders the window for the README and pull requests, from the real view, on the
//! core's made-up demo account. Debug builds only.
//!
//! A screenshot of the running app would show real clients, so the picture is
//! drawn here instead: the window is filled from the demo account, rendered to a
//! PNG at twice its size, and the app quits. GTK needs a display to lay anything
//! out, so the window does appear, for a moment.
//!
//! `SHEAVES_RENDER=docs/images cargo run -p sheaves-gnome`

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use sheaves_core::credentials::MemoryCredentialStore;
use sheaves_core::ffi::{Tracker, TrackerObserver, TrackerSnapshot};

use crate::window::Window;

struct Ignore;

impl TrackerObserver for Ignore {
    fn state_changed(&self, _: TrackerSnapshot) {}
}

pub fn render(directory: PathBuf) -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id("com.rainhead.Sheaves.Documentation")
        .build();
    // GApplication reports success however the work went, so a failure to write
    // is carried out separately for the exit status.
    let failed = std::rc::Rc::new(std::cell::Cell::new(false));
    let activated = failed.clone();
    app.connect_activate(move |app| {
        let failed = activated.clone();
        // Pinned, so the images do not follow whatever the desktop is set to.
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceLight);
        let tracker = Tracker::new(
            String::new(),
            true,
            Arc::new(MemoryCredentialStore::default()),
            Arc::new(Ignore),
        );
        let (actions, _ignored) = async_channel::unbounded();
        let window = Window::new(app, actions);
        let directory = directory.clone();
        let app = app.clone();
        let failed = failed.clone();
        glib::spawn_future_local(async move {
            tracker.sync().await;
            window.render(&tracker.snapshot());
            window.window.present();
            // A moment for the window to be mapped and laid out.
            glib::timeout_future(Duration::from_millis(600)).await;
            let content = window.window.content().expect("the window has content");
            report(
                write(&content, &window, &directory.join("gnome-window.png")),
                &failed,
            );

            // The absence prompt, over the window, for an hour away from the
            // demo account's running timer.
            let snapshot = tracker.snapshot();
            if let Some(entry) = snapshot.running_entry.clone() {
                let now = jiff::Timestamp::now();
                let began = entry.timer_started_at.map_or(now, |started| {
                    started.max(now - jiff::SignedDuration::from_mins(70))
                });
                let absence = sheaves_core::absence::Absence {
                    began,
                    ended: now - jiff::SignedDuration::from_mins(5),
                };
                let wording = crate::prompt::Wording::of(
                    &absence,
                    &entry,
                    snapshot.state.today,
                    crate::window::hours_format(&snapshot),
                    false,
                );
                let parent: gtk::Widget = window.window.clone().upcast();
                let dialog = crate::prompt::present(Some(&parent), &wording, Vec::new(), |_| {});
                glib::timeout_future(Duration::from_millis(600)).await;
                report(
                    write(&dialog, &window, &directory.join("gnome-prompt.png")),
                    &failed,
                );
            }
            app.quit();
        });
    });
    let status = app.run_with_args::<&str>(&[]);
    if failed.get() {
        glib::ExitCode::FAILURE
    } else {
        status
    }
}

fn report(written: Result<PathBuf, String>, failed: &std::cell::Cell<bool>) {
    match written {
        Ok(path) => println!("{}", path.display()),
        Err(error) => {
            eprintln!("could not render: {error}");
            failed.set(true);
        }
    }
}

/// Draws `widget`, shown in `window`, to a PNG at twice its size.
fn write(
    widget: &impl IsA<gtk::Widget>,
    window: &Rc<Window>,
    path: &std::path::Path,
) -> Result<PathBuf, String> {
    let (width, height) = (widget.width() as f32, widget.height() as f32);
    let paintable = gtk::WidgetPaintable::new(Some(widget));
    let snapshot = gtk::Snapshot::new();
    snapshot.scale(2.0, 2.0);
    paintable.snapshot(&snapshot, width.into(), height.into());
    let node = snapshot.to_node().ok_or("nothing was drawn")?;
    let renderer = window
        .window
        .renderer()
        .ok_or("the window has no renderer")?;
    let texture = renderer.render_texture(
        &node,
        Some(&gtk::graphene::Rect::new(
            0.0,
            0.0,
            width * 2.0,
            height * 2.0,
        )),
    );
    std::fs::create_dir_all(path.parent().unwrap_or(path)).map_err(|error| error.to_string())?;
    texture
        .save_to_png(path)
        .map_err(|error| error.to_string())?;
    Ok(path.to_path_buf())
}
