//! The application: one instance per session, the core's tracker, the item in the
//! top bar, and the window, joined by channels so each runs where it must. GTK
//! wants its main thread; the tracker and the D-Bus item run on Tokio.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gio, glib};
use sheaves_core::ffi::{Tracker, TrackerObserver, TrackerSnapshot};
use sheaves_core::persistence;
use sheaves_core::tracker::{Activity, Connection};

use crate::indicator::{self, Command, Indicator, Look};
use crate::keyring::Keyring;
use crate::settings;
use crate::window::{self, Action, Window};

pub const APP_ID: &str = "com.rainhead.Sheaves";

/// Carries the core's state reports, which arrive on its threads, to the main
/// thread.
struct Observer(async_channel::Sender<TrackerSnapshot>);

impl TrackerObserver for Observer {
    fn state_changed(&self, snapshot: TrackerSnapshot) {
        let _ = self.0.try_send(snapshot);
    }
}

struct Shell {
    app: adw::Application,
    tracker: Arc<Tracker>,
    window: Rc<Window>,
    latest: RefCell<TrackerSnapshot>,
    looks: tokio::sync::watch::Sender<Look>,
}

pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    let shell: Rc<RefCell<Option<Rc<Shell>>>> = Rc::default();

    {
        let shell = shell.clone();
        app.connect_startup(move |app| {
            *shell.borrow_mut() = Some(start(app));
        });
    }
    {
        let shell = shell.clone();
        // Every launch arrives here, the first and each later one, which GApplication
        // hands to the instance already running. Phase 9's quick entry joins in here.
        app.connect_command_line(move |_, command_line| {
            let background = command_line
                .arguments()
                .iter()
                .any(|argument| argument.to_str() == Some("--background"));
            if !background && let Some(shell) = shell.borrow().as_ref() {
                shell.window.window.present();
            }
            glib::ExitCode::SUCCESS
        });
    }
    app.run()
}

fn start(app: &adw::Application) -> Rc<Shell> {
    // The top bar item keeps the app alive with no window open.
    std::mem::forget(app.hold());

    // So GNOME shows Sheaves' name and icon for its window, not a generic gear.
    gtk::Window::set_default_icon_name(APP_ID);
    if let Err(error) = crate::autostart::this_program()
        .and_then(|program| crate::launcher::install(&glib::user_data_dir(), &program))
    {
        log::warn!("could not install Sheaves' desktop entry: {error}");
    }

    let (snapshots, snapshot_rx) = async_channel::unbounded();
    let directory = persistence::data_directory();
    let tracker = Tracker::new(
        directory.to_string_lossy().into_owned(),
        false,
        Arc::new(Keyring),
        Arc::new(Observer(snapshots)),
    );

    let (actions, action_rx) = async_channel::unbounded();
    let window = Window::new(app, actions);
    let initial = tracker.snapshot();
    window.render(&initial);

    let (commands, command_rx) = async_channel::unbounded();
    let (looks, look_rx) = tokio::sync::watch::channel(Look::default());
    start_indicator(commands, look_rx);

    let shell = Rc::new(Shell {
        app: app.clone(),
        tracker: tracker.clone(),
        window,
        latest: RefCell::new(initial),
        looks,
    });
    shell.show_look();

    glib::spawn_future_local({
        let shell = shell.clone();
        async move {
            while let Ok(snapshot) = snapshot_rx.recv().await {
                shell.receive(snapshot);
            }
        }
    });
    glib::spawn_future_local({
        let shell = shell.clone();
        async move {
            while let Ok(action) = action_rx.recv().await {
                shell.act(action);
            }
        }
    });
    glib::spawn_future_local({
        let shell = shell.clone();
        async move {
            while let Ok(command) = command_rx.recv().await {
                shell.obey(command);
            }
        }
    });
    glib::spawn_future_local({
        let shell = shell.clone();
        async move {
            shell.tracker.bootstrap().await;
            // Nothing works without a token, and the top bar is easy to miss on a
            // first launch, so ask for one straight away.
            if shell.tracker.snapshot().state.connection == Connection::NeedsCredentials {
                shell.window.window.present();
                shell.act(Action::OpenSettings);
            }
        }
    });
    shell
}

/// Runs the top bar item on a runtime of its own, showing each look sent on
/// `looks`.
fn start_indicator(
    commands: async_channel::Sender<Command>,
    mut looks: tokio::sync::watch::Receiver<Look>,
) {
    std::thread::Builder::new()
        .name("sheaves-indicator".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime can start");
            runtime.block_on(async move {
                // A missing glyph leaves the item with no icon, not no item.
                let glyph = indicator::install_glyph(&persistence::data_directory().join("icons"))
                    .inspect_err(|error| log::warn!("could not write the top bar glyph: {error}"))
                    .ok();
                let indicator = match Indicator::start(commands, glyph).await {
                    Ok(indicator) => indicator,
                    Err(error) => {
                        log::error!("no item in the top bar: {error}");
                        return;
                    }
                };
                loop {
                    let look = looks.borrow_and_update().clone();
                    if let Err(error) = indicator.show(look).await {
                        log::warn!("could not update the top bar: {error}");
                    }
                    if looks.changed().await.is_err() {
                        return;
                    }
                }
            });
        })
        .expect("a thread can start");
}

impl Shell {
    /// Keeps the newest state: reports can arrive out of order with the state an
    /// action read back on finishing.
    fn receive(&self, snapshot: TrackerSnapshot) {
        if snapshot.state.revision < self.latest.borrow().state.revision {
            return;
        }
        self.window.render(&snapshot);
        *self.latest.borrow_mut() = snapshot;
        self.show_look();
    }

    fn show_look(&self) {
        let latest = self.latest.borrow();
        let look = Look::of(
            &latest.activity,
            window::hours_format(&latest),
            latest.state.now,
        );
        self.looks.send_if_modified(|current| {
            let changed = *current != look;
            *current = look;
            changed
        });
    }

    fn act(self: &Rc<Self>, action: Action) {
        let tracker = self.tracker.clone();
        match action {
            Action::OpenSettings => {
                settings::present(&self.window.window, tracker, &self.latest.borrow());
            }
            Action::Start(target) => {
                glib::spawn_future_local(async move { tracker.start(target, None).await });
            }
            Action::Toggle(entry) => {
                glib::spawn_future_local(async move { tracker.toggle(entry).await });
            }
            Action::ShiftDay(days) => {
                glib::spawn_future_local(async move { tracker.shift_day(days).await });
            }
            Action::Today => {
                glib::spawn_future_local(async move { tracker.go_to_today().await });
            }
            Action::Sync => {
                glib::spawn_future_local(async move { tracker.sync().await });
            }
        };
    }

    fn obey(self: &Rc<Self>, command: Command) {
        match command {
            Command::Open => self.window.window.present(),
            Command::Quit => self.app.quit(),
            Command::StopOrResume => {
                let activity = self.latest.borrow().activity.clone();
                let tracker = self.tracker.clone();
                match activity {
                    Activity::Running(entry) => {
                        glib::spawn_future_local(async move { tracker.stop(entry).await });
                    }
                    Activity::Recent(entry) => {
                        glib::spawn_future_local(async move { tracker.resume(entry).await });
                    }
                    Activity::Idle => self.window.window.present(),
                }
            }
        }
    }
}
