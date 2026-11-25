//! The day in a window: its entries, each with start or stop, the projects to
//! start a timer on, ranked as the core ranks them, and the way to other days.
//!
//! The window draws whatever state the core last reported. Actions go back to the
//! core through [`Action`]; nothing here decides anything.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use sheaves_core::ffi::TrackerSnapshot;
use sheaves_core::hours::HoursFormat;
use sheaves_core::tracked_entry::{EntryId, TimerTarget, TrackedEntry};
use sheaves_core::tracker::Connection;

use crate::budget;
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
    budget: BudgetBar,
    entries: gtk::ListBox,
    no_entries: gtk::Label,
    projects: Rc<ProjectList>,
    status: gtk::Label,
    /// The hours label of each entry row, so the running one can tick without
    /// rebuilding the list under the pointer.
    hours: RefCell<Vec<(EntryId, gtk::Label)>>,
    shown_entries: RefCell<Vec<TrackedEntry>>,
    actions: async_channel::Sender<Action>,
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
        let budget = BudgetBar::new();
        column.append(&budget.root);
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

        Rc::new(Self {
            window,
            day,
            total,
            today,
            banner,
            budget,
            entries,
            no_entries,
            projects,
            status,
            hours: RefCell::default(),
            shown_entries: RefCell::default(),
            actions,
        })
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

        self.budget.render(snapshot);
        self.projects.show(snapshot);
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
        self.entries.remove_all();
        let mut hours = Vec::new();
        for entry in entries {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&entry.task.name))
                .subtitle(glib::markup_escape_text(&subtitle(entry)))
                // The project on the first line and up to two of notes.
                .subtitle_lines(3)
                .activatable(!entry.is_locked)
                .build();
            if entry.is_pending {
                // A local change Harvest has not acknowledged yet.
                let waiting = gtk::Image::from_icon_name("emblem-synchronizing-symbolic");
                waiting.set_tooltip_text(Some("Waiting to sync"));
                row.add_prefix(&waiting);
            }
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
        self.no_entries.set_visible(entries.is_empty());
        *self.hours.borrow_mut() = hours;
    }
}

/// The budget of the project on the clock: how much is left and how much is gone.
/// It shows nothing unless there is a readable budget for that project, as on the
/// Mac, so an account without budgets has no empty bar.
struct BudgetBar {
    root: gtk::Box,
    project: gtk::Label,
    left: gtk::Label,
    bar: gtk::ProgressBar,
}

impl BudgetBar {
    fn new() -> Self {
        install_css();
        let project = gtk::Label::new(None);
        project.add_css_class("caption");
        project.add_css_class("dim-label");
        project.set_xalign(0.0);
        project.set_hexpand(true);
        project.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let left = gtk::Label::new(None);
        left.add_css_class("caption");
        left.add_css_class("numeric");
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        line.append(&project);
        line.append(&left);
        let bar = gtk::ProgressBar::new();
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.append(&line);
        root.append(&bar);
        root.set_visible(false);
        Self {
            root,
            project,
            left,
            bar,
        }
    }

    fn render(&self, snapshot: &TrackerSnapshot) {
        let shown = snapshot.running_entry.as_ref().and_then(|entry| {
            let budget = snapshot.state.budget_for(entry)?;
            let format = hours_format(snapshot);
            Some((entry, budget, budget::left(budget, format)?, format))
        });
        let Some((entry, budget, left, format)) = shown else {
            self.root.set_visible(false);
            return;
        };
        let detail = budget::detail(&entry.project.name, budget, format);
        self.project.set_label(&entry.project.name);
        self.left.set_label(&left.text);
        self.bar.set_fraction(budget.fraction_used());
        for widget in [self.left.upcast_ref::<gtk::Widget>(), self.bar.upcast_ref()] {
            if left.over {
                widget.add_css_class("budget-over");
            } else {
                widget.remove_css_class("budget-over");
            }
        }
        self.root.set_tooltip_text(Some(&detail));
        self.root
            .update_property(&[gtk::accessible::Property::Label(&detail)]);
        self.root.set_visible(true);
    }
}

/// Colours the bar and the figure red once the budget is overspent.
fn install_css() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        "progressbar.budget-over > trough > progress { background-color: @error_color; }\n\
         label.budget-over { color: @error_color; }",
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
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

/// The project and its client on the first line, and the notes beneath:
/// "123 Industries · Online Store\nCheckout flow".
fn subtitle(entry: &TrackedEntry) -> String {
    let mut subtitle = entry.target().project_label();
    if let Some(notes) = entry
        .notes
        .as_deref()
        .filter(|notes| !notes.trim().is_empty())
    {
        subtitle.push('\n');
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

    fn entry(notes: Option<&str>) -> TrackedEntry {
        TrackedEntry {
            id: EntryId::Server(1),
            project: Reference::new(1, "Online Store"),
            task: Reference::new(2, "Programming"),
            client: Some(Reference::new(3, "123 Industries")),
            notes: notes.map(str::to_string),
            spent_date: CalendarDate::new(2026, 11, 4),
            banked_hours: 1.5,
            is_running: false,
            timer_started_at: None,
            is_locked: false,
            is_billable: true,
            is_pending: false,
            updated_at: Timestamp::UNIX_EPOCH,
        }
    }

    #[test]
    fn notes_sit_beneath_the_project() {
        assert_eq!(
            subtitle(&entry(Some(" Checkout flow\n"))),
            "123 Industries · Online Store\nCheckout flow"
        );
    }

    #[test]
    fn no_notes_leave_only_the_project() {
        assert_eq!(subtitle(&entry(None)), "123 Industries · Online Store");
        assert_eq!(subtitle(&entry(Some("  "))), "123 Industries · Online Store");
    }
}
