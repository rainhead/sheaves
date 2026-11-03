//! Asks what to do about a timer that ran while nobody was here, as the Mac's
//! absence prompt does (`Apps/Mac/Sources/AbsencePrompt.swift`), in the same words.
//!
//! It never decides on its own: rewriting somebody's hours without asking would be
//! a worse fault than the one it fixes.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use jiff::Timestamp;
use sheaves_core::absence::{Absence, AbsenceResolution};
use sheaves_core::calendar_date::CalendarDate;
use sheaves_core::hours::HoursFormat;
use sheaves_core::tracked_entry::{TimerTarget, TrackedEntry};

use crate::format;

/// How a prompt ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Resolved(AbsenceResolution),
    /// Closed unanswered: "Not Now", Escape, or withdrawn because it no longer
    /// applies.
    Dismissed,
}

/// What the prompt says, worked out apart from the widgets so it can be tested.
#[derive(Clone, Debug, PartialEq)]
pub struct Wording {
    pub heading: String,
    pub body: String,
    /// The choices in the order shown, each with its title and what it does.
    pub choices: Vec<(Choice, String, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    TrimAndContinue,
    TrimAndStop,
    LogSeparately,
    Keep,
}

impl Wording {
    pub fn of(
        absence: &Absence,
        entry: &TrackedEntry,
        today: CalendarDate,
        format: HoursFormat,
        twelve_hour: bool,
    ) -> Self {
        // The entry belongs to a day that is over, so carrying on would bank
        // today's work onto yesterday's date: stopping is the sound default then.
        let day_is_over = entry.spent_date != today;
        let since = clock_time(absence.began, day_is_over, twelve_hour);
        let away = format::hours(absence.duration() / 3600.0, format);
        let trim = match absence.trimmed_hours(entry) {
            Some(hours) => format!("Leaves {} on this entry", format::hours(hours, format)),
            None => "Takes the time away off this entry".to_string(),
        };
        let mut choices = if day_is_over {
            vec![
                (Choice::TrimAndStop, "Trim and stop".to_string(), trim),
                (
                    Choice::TrimAndContinue,
                    "Trim, keep timing".to_string(),
                    "Carries on from when you came back".to_string(),
                ),
            ]
        } else {
            vec![
                (
                    Choice::TrimAndContinue,
                    "Trim, keep timing".to_string(),
                    trim,
                ),
                (
                    Choice::TrimAndStop,
                    "Trim and stop".to_string(),
                    "Ends the entry where you left".to_string(),
                ),
            ]
        };
        choices.push((
            Choice::LogSeparately,
            "Log the time away separately…".to_string(),
            if day_is_over {
                "Books it to the meeting you were in, and stops this timer"
            } else {
                "Books it to the meeting you were in, and keeps timing"
            }
            .to_string(),
        ));
        choices.push((
            Choice::Keep,
            "Keep it".to_string(),
            "It was working time after all".to_string(),
        ));
        Self {
            heading: "This timer ran while you were away".to_string(),
            body: format!(
                "{} · {}\nAway for {away}, since {since}",
                entry.task.name,
                entry.target().project_label()
            ),
            choices,
        }
    }
}

/// `instant` as a time of day in this machine's zone, with the date as well once
/// it is not today: "since 5:12 PM" is a riddle the following morning.
fn clock_time(instant: Timestamp, with_date: bool, twelve_hour: bool) -> String {
    let Ok(time) = gtk::glib::DateTime::from_unix_local(instant.as_second()) else {
        return instant.to_string();
    };
    let clock = if twelve_hour { "%-l:%M %p" } else { "%H:%M" };
    let pattern = if with_date {
        format!("%e %b, {clock}")
    } else {
        clock.to_string()
    };
    time.format(&pattern)
        .map(|text| text.trim().to_string())
        .unwrap_or_else(|_| instant.to_string())
}

/// Whether GNOME is set to show a 12-hour clock.
pub fn twelve_hour_clock() -> bool {
    let schema = "org.gnome.desktop.interface";
    gtk::gio::SettingsSchemaSource::default()
        .and_then(|source| source.lookup(schema, true))
        .is_some_and(|_| gtk::gio::Settings::new(schema).string("clock-format") == "12h")
}

/// Shows the prompt over `parent`, or in a window of its own when `parent` is
/// hidden, and calls `finished` once with how it ended.
pub fn present(
    parent: Option<&gtk::Widget>,
    wording: &Wording,
    targets: Vec<TimerTarget>,
    finished: impl Fn(Outcome) + 'static,
) -> adw::AlertDialog {
    let dialog = adw::AlertDialog::new(Some(&wording.heading), Some(&wording.body));
    dialog.add_response("later", "Not Now");
    dialog.set_close_response("later");

    let finished = Rc::new(finished);
    let answered = Rc::new(Cell::new(false));
    let answer = {
        let dialog = dialog.clone();
        let finished = finished.clone();
        let answered = answered.clone();
        Rc::new(move |resolution: AbsenceResolution| {
            if answered.replace(true) {
                return;
            }
            finished(Outcome::Resolved(resolution));
            dialog.force_close();
        })
    };
    {
        let finished = finished.clone();
        dialog.connect_closed(move |_| {
            if !answered.replace(true) {
                finished(Outcome::Dismissed);
            }
        });
    }

    let stack = gtk::Stack::new();
    stack.set_vhomogeneous(false);
    let choices = gtk::ListBox::new();
    choices.add_css_class("boxed-list");
    choices.set_selection_mode(gtk::SelectionMode::None);
    for (index, (choice, title, detail)) in wording.choices.iter().enumerate() {
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(title))
            .subtitle(glib::markup_escape_text(detail))
            .activatable(true)
            .build();
        if index == 0 {
            row.add_css_class("accent");
        }
        let choice = *choice;
        let answer = answer.clone();
        let stack = stack.clone();
        row.connect_activated(move |_| match choice {
            Choice::TrimAndContinue => answer(AbsenceResolution::TrimAndContinue),
            Choice::TrimAndStop => answer(AbsenceResolution::TrimAndStop),
            Choice::Keep => answer(AbsenceResolution::Keep),
            Choice::LogSeparately => stack.set_visible_child_name("targets"),
        });
        choices.append(&row);
    }
    stack.add_named(&choices, Some("choices"));
    stack.add_named(&target_picker(targets, answer), Some("targets"));
    stack.set_visible_child_name("choices");
    dialog.set_extra_child(Some(&stack));
    dialog.present(parent);
    dialog
}

/// "Log the time away to…": the ranked targets, searchable.
fn target_picker(targets: Vec<TimerTarget>, answer: Rc<dyn Fn(AbsenceResolution)>) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let label = gtk::Label::new(Some("Log the time away to…"));
    label.add_css_class("heading");
    label.set_xalign(0.0);
    let search = gtk::SearchEntry::new();
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    let haystacks: Vec<String> = targets
        .iter()
        .map(|target| format!("{} {}", target.task.name, target.project_label()).to_lowercase())
        .collect();
    for target in targets {
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&target.task.name))
            .subtitle(glib::markup_escape_text(&target.project_label()))
            .activatable(true)
            .build();
        let answer = answer.clone();
        row.connect_activated(move |_| answer(AbsenceResolution::TrimAndLog(target.clone())));
        list.append(&row);
    }
    {
        let search = search.clone();
        list.set_filter_func(move |row| {
            let query = search.text().to_lowercase();
            haystacks.get(row.index() as usize).is_some_and(|haystack| {
                query.split_whitespace().all(|word| haystack.contains(word))
            })
        });
    }
    {
        let list = list.clone();
        search.connect_search_changed(move |_| list.invalidate_filter());
    }
    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_hscrollbar_policy(gtk::PolicyType::Never);
    scrolled.set_min_content_height(240);
    scrolled.set_child(Some(&list));
    column.append(&label);
    column.append(&search);
    column.append(&scrolled);
    column
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheaves_core::models::Reference;
    use sheaves_core::tracked_entry::EntryId;

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn running(day: CalendarDate) -> TrackedEntry {
        TrackedEntry {
            id: EntryId::Server(1),
            project: Reference::new(1, "Online Store"),
            task: Reference::new(2, "Programming"),
            client: Some(Reference::new(3, "123 Industries")),
            notes: None,
            spent_date: day,
            banked_hours: 0.0,
            is_running: true,
            timer_started_at: Some(at("2026-11-03T09:00:00Z")),
            is_locked: false,
            is_billable: true,
            is_pending: false,
            updated_at: at("2026-11-03T09:00:00Z"),
        }
    }

    fn lunch() -> Absence {
        Absence {
            began: at("2026-11-03T12:00:00Z"),
            ended: at("2026-11-03T13:00:00Z"),
        }
    }

    #[test]
    fn says_what_ran_how_long_and_what_trimming_leaves() {
        let today = CalendarDate::new(2026, 11, 3);
        let wording = Wording::of(
            &lunch(),
            &running(today),
            today,
            HoursFormat::HoursMinutes,
            false,
        );
        assert!(
            wording
                .body
                .starts_with("Programming · 123 Industries · Online Store\nAway for 1:00, since ")
        );
        assert_eq!(wording.choices[0].0, Choice::TrimAndContinue);
        assert_eq!(wording.choices[0].2, "Leaves 3:00 on this entry");
        assert_eq!(
            wording
                .choices
                .iter()
                .map(|(choice, _, _)| *choice)
                .collect::<Vec<_>>(),
            [
                Choice::TrimAndContinue,
                Choice::TrimAndStop,
                Choice::LogSeparately,
                Choice::Keep
            ]
        );
    }

    /// Carrying on would bank today's work onto a day that is over, so stopping
    /// comes first, and logging separately says it stops this timer.
    #[test]
    fn on_a_day_that_is_over_stopping_comes_first() {
        let yesterday = CalendarDate::new(2026, 11, 2);
        let wording = Wording::of(
            &lunch(),
            &running(yesterday),
            CalendarDate::new(2026, 11, 3),
            HoursFormat::HoursMinutes,
            false,
        );
        assert_eq!(wording.choices[0].0, Choice::TrimAndStop);
        assert!(wording.choices[2].2.ends_with("and stops this timer"));
    }
}
