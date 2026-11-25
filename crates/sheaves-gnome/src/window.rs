//! The day in a window: its entries, each with start or stop, the projects to
//! start a timer on, ranked as the core ranks them, and the way to other days.
//!
//! The window draws whatever state the core last reported. Actions go back to the
//! core through [`Action`]; nothing here decides anything.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use sheaves_core::ffi::TrackerSnapshot;
use sheaves_core::hours::HoursFormat;
use sheaves_core::tracked_entry::{EntryId, TimerTarget, TrackedEntry};
use sheaves_core::tracker::Connection;

use crate::format;
use crate::projects::ProjectList;

/// What the user asked for in the window.
#[derive(Clone, Debug)]
pub enum Action {
    Start(TimerTarget),
    Toggle(TrackedEntry),
    /// Open the editor for an entry's notes and total.
    Edit(TrackedEntry),
    /// Log time on the day on screen without a timer.
    LogTime,
    /// Ask to delete an entry, after the same confirmation as the editor's Delete.
    Delete(TrackedEntry),
    ShiftDay(i32),
    Today,
    Sync,
    OpenSettings,
}

pub struct Window {
    pub window: adw::ApplicationWindow,
    day: gtk::Label,
    total: gtk::Label,
    today: gtk::Label,
    banner: adw::Banner,
    entries: gtk::ListBox,
    no_entries: gtk::Label,
    projects: Rc<ProjectList>,
    status: gtk::Label,
    /// The hours label of each entry row, so the running one can tick without
    /// rebuilding the list under the pointer.
    hours: RefCell<Vec<(EntryId, gtk::Label)>>,
    shown_entries: RefCell<Vec<TrackedEntry>>,
    actions: async_channel::Sender<Action>,
    /// Only on offer on a day other than today.
    go_today: gio::SimpleAction,
    /// Until the first entries are drawn, the window has no better place to put
    /// the keyboard than on the first of them.
    focus_first: Cell<bool>,
}

/// The entry that ⌫ or Delete asks to delete when the row at `index` has the
/// keyboard. A locked entry can't be deleted, in Harvest or here, as on the Mac.
fn deletable(entries: &[TrackedEntry], index: i32) -> Option<&TrackedEntry> {
    let index = usize::try_from(index).ok()?;
    entries.get(index).filter(|entry| !entry.is_locked)
}

impl Window {
    pub fn new(app: &adw::Application, actions: async_channel::Sender<Action>) -> Rc<Self> {
        let back = gtk::Button::from_icon_name("go-previous-symbolic");
        back.set_tooltip_text(Some("Previous day"));
        let forward = gtk::Button::from_icon_name("go-next-symbolic");
        forward.set_tooltip_text(Some("Next day"));
        // The day between its arrows, as the Mac's panel has it, rather than the
        // arrows at the window's edge and the day adrift in the middle. Beneath
        // it the day's total, and on any other day a way back to today, which
        // as a button of its own crowded a long date.
        let day = gtk::Label::new(None);
        day.add_css_class("heading");
        day.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let total = gtk::Label::new(None);
        total.add_css_class("caption");
        total.add_css_class("dim-label");
        total.set_ellipsize(gtk::pango::EllipsizeMode::End);
        // A link in a label rather than a link button, whose padding made the
        // header taller and the window wider.
        let today = gtk::Label::new(None);
        today.set_markup("<a href=\"today\">Back to today</a>");
        today.add_css_class("caption");
        today.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let below = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        below.set_halign(gtk::Align::Center);
        below.append(&total);
        below.append(&today);
        // A fixed width, so the arrows stay where they are as the day's name and
        // total change length: clicking back twice would otherwise miss the
        // second time. At least this wide, and clamped to it, with every label
        // in it shortened rather than allowed to widen it.
        const DAY_WIDTH: i32 = 170;
        let named = gtk::Box::new(gtk::Orientation::Vertical, 0);
        named.set_valign(gtk::Align::Center);
        named.set_size_request(DAY_WIDTH, -1);
        named.append(&day);
        named.append(&below);
        back.set_valign(gtk::Align::Center);
        forward.set_valign(gtk::Align::Center);
        let title = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        title.append(&back);
        let clamped = adw::Clamp::builder()
            .maximum_size(DAY_WIDTH)
            .tightening_threshold(DAY_WIDTH)
            .child(&named)
            .build();
        title.append(&clamped);
        title.append(&forward);
        let log_time = gtk::Button::from_icon_name("list-add-symbolic");
        log_time.set_tooltip_text(Some("Log time without a timer"));
        let settings = gtk::Button::from_icon_name("emblem-system-symbolic");
        settings.set_tooltip_text(Some("Settings"));

        let header = adw::HeaderBar::new();
        // Only + beside the day, at the start where GNOME puts "add": with
        // Settings there too, and the window's own buttons, the day could not be
        // centred at the window's usual width. Settings is in the footer, as on
        // the Mac.
        header.set_title_widget(Some(&title));
        header.pack_start(&log_time);

        let banner = adw::Banner::new("");

        let entries = gtk::ListBox::new();
        entries.add_css_class("boxed-list");
        entries.set_selection_mode(gtk::SelectionMode::None);
        let no_entries = gtk::Label::new(Some("Nothing logged on this day."));
        no_entries.add_css_class("dim-label");

        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Find a project or task"));
        let projects = {
            let actions = actions.clone();
            ProjectList::new(&search, move |target| {
                let _ = actions.try_send(Action::Start(target));
            })
        };

        let status = gtk::Label::new(None);
        status.add_css_class("dim-label");
        status.add_css_class("caption");
        status.set_xalign(0.0);

        let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
        column.set_margin_top(12);
        column.set_margin_bottom(12);
        column.set_margin_start(12);
        column.set_margin_end(12);
        column.append(&entries);
        column.append(&no_entries);
        column.append(&heading("Start a timer"));
        column.append(&search);
        column.append(&projects.list);
        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(560);
        clamp.set_child(Some(&column));
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_hscrollbar_policy(gtk::PolicyType::Never);
        scrolled.set_vexpand(true);
        scrolled.set_child(Some(&clamp));

        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        footer.set_margin_start(12);
        footer.set_margin_end(12);
        footer.set_margin_top(6);
        footer.set_margin_bottom(6);
        status.set_hexpand(true);
        settings.add_css_class("flat");
        footer.append(&status);
        footer.append(&settings);

        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.append(&banner);
        body.append(&scrolled);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&body));
        toolbar.add_bottom_bar(&footer);

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Sheaves")
            .default_width(440)
            .default_height(680)
            .content(&toolbar)
            // The app lives in the top bar; closing the window only hides it.
            .hide_on_close(true)
            .build();

        let send = |action: Action| {
            let actions = actions.clone();
            move |_: &gtk::Button| {
                let _ = actions.try_send(action.clone());
            }
        };
        back.connect_clicked(send(Action::ShiftDay(-1)));
        forward.connect_clicked(send(Action::ShiftDay(1)));
        {
            let actions = actions.clone();
            // A link with nowhere to go: the click is the action.
            today.connect_activate_link(move |_, _| {
                let _ = actions.try_send(Action::Today);
                glib::Propagation::Stop
            });
        }
        settings.connect_clicked(send(Action::OpenSettings));
        log_time.connect_clicked(send(Action::LogTime));
        {
            let actions = actions.clone();
            banner.connect_button_clicked(move |banner| {
                let action = if banner.button_label().as_deref() == Some("Settings") {
                    Action::OpenSettings
                } else {
                    Action::Sync
                };
                let _ = actions.try_send(action);
            });
        }

        let go_today = gio::SimpleAction::new("today", None);
        let this = Rc::new(Self {
            window,
            day,
            total,
            today,
            banner,
            entries,
            no_entries,
            projects,
            status,
            hours: RefCell::default(),
            shown_entries: RefCell::default(),
            actions,
            go_today,
            focus_first: Cell::new(true),
        });
        this.install_keys();
        this
    }

    /// The keyboard. Shortcuts that work anywhere in the window are actions with
    /// accelerators, so GNOME's shortcut conventions and any future shortcuts
    /// window see them. Alt+arrows rather than Ctrl+arrows for the day, as in
    /// GNOME's back and forward, because Ctrl+arrows move by word in the search
    /// field. ↑ ↓ and ⏎ on the entries are the list box's own.
    fn install_keys(self: &Rc<Self>) {
        let window = &self.window;
        let app = window.application().expect("the window has an application");
        let shortcuts = [
            ("log-time", Action::LogTime, "<Control>n"),
            ("settings", Action::OpenSettings, "<Control>comma"),
            ("previous-day", Action::ShiftDay(-1), "<Alt>Left"),
            ("next-day", Action::ShiftDay(1), "<Alt>Right"),
        ];
        for (name, action, accelerator) in shortcuts {
            let simple = gio::SimpleAction::new(name, None);
            self.when_free(&simple, action);
            window.add_action(&simple);
            app.set_accels_for_action(&format!("win.{name}"), &[accelerator]);
        }
        self.when_free(&self.go_today, Action::Today);
        window.add_action(&self.go_today);
        app.set_accels_for_action("win.today", &["<Alt>Home"]);

        let keys = gtk::EventControllerKey::new();
        {
            let this = Rc::downgrade(self);
            keys.connect_key_pressed(move |_, key, _, state| {
                let Some(this) = this.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if !state.is_empty() || this.window.visible_dialog().is_some() {
                    return glib::Propagation::Proceed;
                }
                if key == gdk::Key::Escape {
                    this.window.close();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
        self.window.add_controller(keys);

        // On the list of entries only, so a text field elsewhere keeps its own
        // Delete and BackSpace.
        let keys = gtk::EventControllerKey::new();
        {
            let this = Rc::downgrade(self);
            keys.connect_key_pressed(move |_, key, _, state| {
                let Some(this) = this.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let delete = matches!(
                    key,
                    gdk::Key::Delete | gdk::Key::KP_Delete | gdk::Key::BackSpace
                );
                if !delete || !state.is_empty() {
                    return glib::Propagation::Proceed;
                }
                let index = this
                    .entries
                    .focus_child()
                    .and_downcast::<gtk::ListBoxRow>()
                    .map_or(-1, |row| row.index());
                let shown = this.shown_entries.borrow();
                match deletable(&shown, index) {
                    Some(entry) => {
                        let _ = this.actions.try_send(Action::Delete(entry.clone()));
                        glib::Propagation::Stop
                    }
                    None => glib::Propagation::Proceed,
                }
            });
        }
        self.entries.add_controller(keys);
    }

    /// Sends `action` when `simple` is activated and no dialog is over the window,
    /// whose own fields and buttons should not meet a shortcut meant for the day.
    fn when_free(self: &Rc<Self>, simple: &gio::SimpleAction, action: Action) {
        let this = Rc::downgrade(self);
        simple.connect_activate(move |_, _| {
            if let Some(this) = this.upgrade()
                && this.window.visible_dialog().is_none()
            {
                let _ = this.actions.try_send(action.clone());
            }
        });
    }

    pub fn render(&self, snapshot: &TrackerSnapshot) {
        let state = &snapshot.state;
        let format = hours_format(snapshot);

        let day = if snapshot.is_today {
            "Today".to_string()
        } else {
            format::day(state.day)
        };
        self.day.set_label(&day);
        self.total.set_label(&format!(
            "{} logged",
            format::hours(snapshot.total_hours, format)
        ));
        self.today.set_visible(!snapshot.is_today);
        self.go_today.set_enabled(!snapshot.is_today);

        self.render_banner(snapshot);
        self.status.set_label(&status_text(snapshot));

        if *self.shown_entries.borrow() != state.entries {
            self.rebuild_entries(&state.entries);
            *self.shown_entries.borrow_mut() = state.entries.clone();
        }
        for (id, label) in self.hours.borrow().iter() {
            if let Some(entry) = state.entries.iter().find(|entry| entry.id == *id) {
                label.set_label(&format::hours(entry.hours(state.now), format));
            }
        }

        self.projects.show(&snapshot.ordered_projects);
    }

    fn render_banner(&self, snapshot: &TrackerSnapshot) {
        let (title, button) = match &snapshot.state.connection {
            Connection::NeedsCredentials => (
                Some("Connect to Harvest to see your timesheet.".to_string()),
                Some("Settings"),
            ),
            Connection::Offline { reason } => (Some(format!("Offline: {reason}")), Some("Retry")),
            Connection::Connecting | Connection::Online => (snapshot.notice.clone(), None),
        };
        // A banner reads its title as markup, and the reason is error text, which
        // can hold an `&` or a `<`.
        self.banner
            .set_title(&glib::markup_escape_text(title.as_deref().unwrap_or("")));
        self.banner.set_button_label(button);
        self.banner.set_revealed(title.is_some());
    }

    fn rebuild_entries(&self, entries: &[TrackedEntry]) {
        // Rebuilding drops the row that had the keyboard, which would send it back
        // to the top of the window in the middle of a run of arrow presses.
        let focused = self
            .entries
            .focus_child()
            .and_downcast::<gtk::ListBoxRow>()
            .map(|row| row.index());
        self.entries.remove_all();
        let mut hours = Vec::new();
        for entry in entries {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&entry.task.name))
                .subtitle(glib::markup_escape_text(&subtitle(entry)))
                .activatable(!entry.is_locked)
                .build();
            if entry.is_locked {
                // Approved or invoiced in Harvest: it can be looked at, not changed.
                let lock = gtk::Image::from_icon_name("changes-prevent-symbolic");
                lock.set_tooltip_text(Some("Locked in Harvest"));
                row.add_prefix(&lock);
            } else {
                let actions = self.actions.clone();
                let opened = entry.clone();
                row.connect_activated(move |_| {
                    let _ = actions.try_send(Action::Edit(opened.clone()));
                });
            }
            let label = gtk::Label::new(None);
            label.add_css_class("numeric");
            if entry.is_running {
                label.add_css_class("accent");
            }
            let toggle = gtk::Button::from_icon_name(if entry.is_running {
                "media-playback-stop-symbolic"
            } else {
                "media-playback-start-symbolic"
            });
            toggle.set_tooltip_text(Some(if entry.is_running { "Stop" } else { "Resume" }));
            toggle.add_css_class("flat");
            toggle.set_valign(gtk::Align::Center);
            toggle.set_sensitive(!entry.is_locked);
            let actions = self.actions.clone();
            let clicked = entry.clone();
            toggle.connect_clicked(move |_| {
                let _ = actions.try_send(Action::Toggle(clicked.clone()));
            });
            row.add_suffix(&label);
            row.add_suffix(&toggle);
            self.entries.append(&row);
            hours.push((entry.id, label));
        }
        self.entries.set_visible(!entries.is_empty());
        let keep = focused.or(self.focus_first.get().then_some(0));
        if let (Some(index), false) = (keep, entries.is_empty()) {
            self.focus_first.set(false);
            let last = entries.len() as i32 - 1;
            if let Some(row) = self.entries.row_at_index(index.clamp(0, last)) {
                row.grab_focus();
            }
        }
        self.no_entries.set_visible(entries.is_empty());
        *self.hours.borrow_mut() = hours;
    }
}

fn heading(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("heading");
    label.set_xalign(0.0);
    label.set_margin_top(12);
    label
}

/// The account's format for durations, or hours and minutes before it is known.
pub fn hours_format(snapshot: &TrackerSnapshot) -> HoursFormat {
    HoursFormat::from_time_format(
        snapshot
            .state
            .company
            .as_ref()
            .map(|company| company.time_format.as_str()),
    )
}

/// "123 Industries · Online Store — Checkout flow".
fn subtitle(entry: &TrackedEntry) -> String {
    let mut subtitle = entry.target().project_label();
    if let Some(notes) = entry
        .notes
        .as_deref()
        .filter(|notes| !notes.trim().is_empty())
    {
        subtitle.push_str(" — ");
        subtitle.push_str(notes.trim());
    }
    subtitle
}

/// "Synced 3 minutes ago · 2 changes waiting".
pub fn status_text(snapshot: &TrackerSnapshot) -> String {
    let state = &snapshot.state;
    let mut parts = Vec::new();
    match (&state.connection, state.last_synced_at) {
        (Connection::Connecting, _) => parts.push("Connecting…".to_string()),
        (_, Some(synced)) => {
            let minutes = ((state.now.as_second() - synced.as_second()) / 60).max(0);
            parts.push(match minutes {
                0 => "Synced just now".to_string(),
                1 => "Synced a minute ago".to_string(),
                minutes => format!("Synced {minutes} minutes ago"),
            });
        }
        (_, None) => {}
    }
    match state.pending_count {
        0 => {}
        1 => parts.push("1 change waiting to send".to_string()),
        count => parts.push(format!("{count} changes waiting to send")),
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::Timestamp;
    use sheaves_core::calendar_date::CalendarDate;
    use sheaves_core::models::Reference;

    fn entry(is_locked: bool, is_running: bool) -> TrackedEntry {
        TrackedEntry {
            id: EntryId::Server(1),
            project: Reference::new(1, "Online Store"),
            task: Reference::new(2, "Programming"),
            client: None,
            notes: None,
            spent_date: CalendarDate::new(2026, 11, 25),
            banked_hours: 1.0,
            is_running,
            timer_started_at: None,
            is_locked,
            is_billable: true,
            is_pending: false,
            updated_at: Timestamp::UNIX_EPOCH,
        }
    }

    #[test]
    fn delete_picks_the_focused_entry_unless_it_is_locked() {
        let entries = [entry(false, false), entry(true, false), entry(false, true)];
        assert!(deletable(&entries, 0).is_some());
        assert!(deletable(&entries, 1).is_none(), "a locked entry stays");
        assert!(deletable(&entries, 2).is_some(), "a running one may go");
    }

    #[test]
    fn delete_with_nothing_focused_does_nothing() {
        let entries = [entry(false, false)];
        assert!(deletable(&entries, -1).is_none());
        assert!(deletable(&entries, 1).is_none());
    }
}
