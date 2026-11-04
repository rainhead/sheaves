//! The application: one instance per session, the core's tracker, the item in the
//! top bar, and the window, joined by channels so each runs where it must. GTK
//! wants its main thread; the tracker and the D-Bus item run on Tokio.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gio, glib};
use sheaves_core::absence::{Absence, AbsenceDetector};
use sheaves_core::ffi::{Tracker, TrackerObserver, TrackerSnapshot};
use sheaves_core::persistence;
use sheaves_core::tracked_entry::TrackedEntry;
use sheaves_core::tracker::{Activity, Connection};

use crate::config::{self, Config};
use crate::prompt::{self, Outcome, Wording};

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
    config: RefCell<Config>,
    /// Absences waiting their turn to be asked about, oldest first. Each describes
    /// a different stretch, and answering only the latest would bill every stretch
    /// before it, since a trim keeps everything earlier than where it starts.
    absences: RefCell<VecDeque<Absence>>,
    prompt: RefCell<Prompt>,
}

/// The absence prompt's state.
enum Prompt {
    Idle,
    /// On screen, asking about this absence.
    Asking(Absence, adw::AlertDialog),
    /// Answered, and the answer still being applied. The next question waits, so
    /// it is asked against the entry this one leaves behind.
    Applying,
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

    let config = Config::read(&config::path());
    tracker.set_peers(config.peers.clone());

    // Absences are noticed on a thread of their own and asked about here.
    let (absences, absence_rx) = async_channel::unbounded();
    crate::presence::watch(
        tracker.presence(absence_threshold(), jiff::Timestamp::now()),
        absences,
    );

    let shell = Rc::new(Shell {
        app: app.clone(),
        tracker: tracker.clone(),
        window,
        latest: RefCell::new(initial),
        looks,
        config: RefCell::new(config),
        absences: RefCell::default(),
        prompt: RefCell::new(Prompt::Idle),
    });
    shell.show_look();

    glib::spawn_future_local({
        let shell = shell.clone();
        async move {
            while let Ok(absence) = absence_rx.recv().await {
                shell.absences.borrow_mut().push_back(absence);
                shell.ask_about_next_absence();
            }
        }
    });

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

/// How long with nobody here counts as away. A debug build takes
/// `SHEAVES_ABSENCE_THRESHOLD`, in seconds, so the prompt can be tried at the desk
/// without a quarter of an hour's wait.
fn absence_threshold() -> f64 {
    #[cfg(debug_assertions)]
    if let Some(seconds) = std::env::var("SHEAVES_ABSENCE_THRESHOLD")
        .ok()
        .and_then(|text| text.parse::<f64>().ok())
        .filter(|seconds| *seconds > 0.0)
    {
        log::warn!("absences are {seconds} seconds long in this build");
        return seconds;
    }
    AbsenceDetector::DEFAULT_THRESHOLD
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
        // Takes the question down once it no longer applies: another machine
        // trimmed the timer, which a peer announces at once, or it was stopped
        // somewhere. Asking anyway would trim the same absence twice.
        let withdrawn = match &*self.prompt.borrow() {
            Prompt::Asking(absence, dialog) if !snapshot.state.absence_applies(absence) => {
                Some(dialog.clone())
            }
            _ => None,
        };
        *self.latest.borrow_mut() = snapshot;
        if let Some(dialog) = withdrawn {
            log::info!("absence prompt withdrawn: the timer no longer ran through it");
            // Closing reports a dismissal, which asks about the next absence.
            dialog.force_close();
        }
        self.show_look();
    }

    /// Asks about the oldest absence still waiting, if a timer of ours ran through
    /// it and nothing is being asked already.
    fn ask_about_next_absence(self: &Rc<Self>) {
        if !matches!(*self.prompt.borrow(), Prompt::Idle) {
            return;
        }
        loop {
            let Some(absence) = self.absences.borrow_mut().pop_front() else {
                return;
            };
            let latest = self.latest.borrow().clone();
            let Some(entry) = latest.running_entry.clone() else {
                log::info!("absence ignored: no timer of ours is running");
                continue;
            };
            if !latest.state.absence_applies(&absence) {
                log::info!("absence ignored: no timer of ours was running through it");
                continue;
            }
            let wording = Wording::of(
                &absence,
                &entry,
                latest.state.today,
                window::hours_format(&latest),
                prompt::twelve_hour_clock(),
            );
            let parent = self
                .window
                .window
                .is_visible()
                .then(|| self.window.window.clone().upcast());
            let shell = self.clone();
            let asked = absence;
            let dialog = prompt::present(
                parent.as_ref(),
                &wording,
                latest.state.ordered_targets(),
                move |outcome| shell.answered(asked, entry.clone(), outcome),
            );
            *self.prompt.borrow_mut() = Prompt::Asking(absence, dialog);
            return;
        }
    }

    fn answered(
        self: &Rc<Self>,
        absence: Absence,
        entry: sheaves_core::tracked_entry::TrackedEntry,
        outcome: Outcome,
    ) {
        match outcome {
            Outcome::Dismissed => {
                *self.prompt.borrow_mut() = Prompt::Idle;
                self.ask_about_next_absence();
            }
            Outcome::Resolved(resolution) => {
                *self.prompt.borrow_mut() = Prompt::Applying;
                let shell = self.clone();
                glib::spawn_future_local(async move {
                    shell.tracker.resolve(absence, entry, resolution).await;
                    // The state the answer left, before the next question is
                    // checked against it: its report may not have arrived yet.
                    shell.receive(shell.tracker.snapshot());
                    *shell.prompt.borrow_mut() = Prompt::Idle;
                    shell.ask_about_next_absence();
                });
            }
        }
    }

    fn set_peers(&self, peers: Vec<String>) {
        let mut config = self.config.borrow_mut();
        if config.peers == peers {
            return;
        }
        config.peers = peers;
        self.tracker.set_peers(config.peers.clone());
        if let Err(error) = config.write(&config::path()) {
            log::error!("could not save the peers: {error}");
        }
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
                let shell = self.clone();
                settings::present(
                    &self.window.window,
                    tracker,
                    &self.latest.borrow(),
                    self.config.borrow().peers.clone(),
                    Rc::new(move |peers| shell.set_peers(peers)),
                );
            }
            Action::Start(target) => {
                glib::spawn_future_local(async move { tracker.start(target, None).await });
            }
            Action::Toggle(entry) if entry.is_running => {
                glib::spawn_future_local(async move { tracker.toggle(entry).await });
            }
            Action::Toggle(entry) => self.resume(entry),
            Action::Edit(entry) => {
                let latest = self.latest.borrow();
                let saving = tracker.clone();
                let saved = entry.clone();
                let deleting = tracker.clone();
                let deleted = entry.clone();
                crate::edit::present_editor(
                    &self.window.window,
                    &entry,
                    window::hours_format(&latest),
                    latest.state.now,
                    move |changes| {
                        let tracker = saving.clone();
                        let entry = saved.clone();
                        glib::spawn_future_local(async move {
                            if let Some(notes) = changes.notes {
                                tracker.update_notes(entry.clone(), notes).await;
                            }
                            if let Some(hours) = changes.hours {
                                tracker.update_hours(entry, hours).await;
                            }
                        });
                    },
                    move || {
                        let tracker = deleting.clone();
                        let entry = deleted.clone();
                        glib::spawn_future_local(async move { tracker.delete(entry).await });
                    },
                );
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

    /// Resumes `entry`, asking first when it is from a day that is over: resuming
    /// it would add today's work to that day. Both the window and the top bar come
    /// here, since just after midnight the top bar can still offer yesterday's.
    fn resume(self: &Rc<Self>, entry: TrackedEntry) {
        let tracker = self.tracker.clone();
        if entry.spent_date == self.latest.borrow().state.today {
            glib::spawn_future_local(async move { tracker.resume(entry).await });
            return;
        }
        self.window.window.present();
        let starting = tracker.clone();
        let target = entry.target();
        let notes = entry.notes.clone();
        let reopened = entry.clone();
        crate::edit::confirm_resume(
            &self.window.window,
            &entry,
            move || {
                // To today first, then start: the core's check for an entry to
                // continue sees only the visible day, and the timer is then where
                // the eye can follow it.
                let tracker = starting.clone();
                let target = target.clone();
                let notes = notes.clone();
                glib::spawn_future_local(async move {
                    tracker.go_to_today().await;
                    tracker.start(target, notes).await;
                });
            },
            move || {
                let tracker = tracker.clone();
                let entry = reopened.clone();
                glib::spawn_future_local(async move { tracker.resume(entry).await });
            },
        );
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
                    Activity::Recent(entry) => self.resume(entry),
                    Activity::Idle => self.window.window.present(),
                }
            }
        }
    }
}
