//! Changing an entry: its project and task, its notes and its total, deleting it,
//! logging time without a timer, and resuming an entry from a day that is over. The questions are the Mac's (`Apps/Mac/Sources/EntryRow.swift`),
//! in its words; the rules are the core's.

use std::rc::Rc;

use adw::prelude::*;
use jiff::Timestamp;
use sheaves_core::hours::{HoursEdit, HoursFormat, parse_hours};
use sheaves_core::tracked_entry::{ProjectTargets, TimerTarget, TrackedEntry};

use crate::chooser::Chooser;
use crate::format;

/// What the editor asks the tracker to change. Each is `None` when it was left as
/// it was, or typed as something that is not a duration.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Changes {
    pub target: Option<TimerTarget>,
    pub notes: Option<String>,
    pub hours: Option<f64>,
}

impl Changes {
    /// What was typed, against what the fields opened with. The total follows the
    /// core's rule (`HoursEdit::commit`): a field still showing what it opened with
    /// asks for nothing, which matters most while a timer runs, since sending the
    /// rounded figure back would restart its measurement and lose the seconds.
    pub fn of(
        entry: &TrackedEntry,
        target: Option<TimerTarget>,
        notes: &str,
        hours: &str,
        hours_as_opened: &str,
        decimal_separator: char,
    ) -> Self {
        let notes = notes.trim();
        Self {
            target: target.filter(|target| target.id() != entry.target().id()),
            notes: (notes != entry.notes.as_deref().unwrap_or("").trim())
                .then(|| notes.to_string()),
            hours: HoursEdit::commit(
                hours,
                hours_as_opened,
                entry.banked_hours,
                entry.is_running,
                decimal_separator,
            ),
        }
    }
}

/// The editor for `entry`: its notes and its total, and a way to delete it.
/// `save` gets what changed; `delete` is called once the deletion is confirmed.
pub fn present_editor(
    parent: &impl IsA<gtk::Widget>,
    entry: &TrackedEntry,
    projects: Vec<ProjectTargets>,
    hours_format: HoursFormat,
    now: Timestamp,
    save: impl Fn(Changes) + 'static,
    delete: impl Fn() + 'static,
) {
    let dialog = adw::AlertDialog::new(
        Some(&entry.task.name),
        Some(&entry.target().project_label()),
    );
    dialog.add_responses(&[("cancel", "Cancel"), ("save", "Save")]);
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");

    let opened = format::hours(entry.hours(now), hours_format);
    let notes = adw::EntryRow::builder()
        .title("Notes")
        .text(entry.notes.as_deref().unwrap_or(""))
        .build();
    let time = adw::EntryRow::builder()
        .title(if entry.is_running {
            "Time so far"
        } else {
            "Time"
        })
        .text(opened.as_str())
        .activates_default(true)
        .build();
    select_all_on_focus(&time);
    let fields = gtk::ListBox::new();
    fields.add_css_class("boxed-list");
    fields.set_selection_mode(gtk::SelectionMode::None);
    let chooser = Chooser::new(projects, Some(&entry.target()));
    fields.append(&chooser.project);
    fields.append(&chooser.task);
    fields.append(&notes);
    fields.append(&time);

    let remove = gtk::Button::with_label("Delete…");
    remove.add_css_class("destructive-action");
    remove.set_halign(gtk::Align::Start);
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.append(&fields);
    column.append(&remove);
    dialog.set_extra_child(Some(&column));

    // Save is unavailable while the time reads as no duration at all: closing
    // would quietly keep the old total, and any notes change without it.
    {
        let dialog = dialog.downgrade();
        let opened = opened.clone();
        time.connect_changed(move |row| {
            let text = row.text();
            let readable =
                text == opened || parse_hours(&text, format::decimal_separator()).is_some();
            if readable {
                row.remove_css_class("error");
            } else {
                row.add_css_class("error");
            }
            if let Some(dialog) = dialog.upgrade() {
                dialog.set_response_enabled("save", readable);
            }
        });
    }

    let delete = Rc::new(delete);
    {
        let dialog = dialog.downgrade();
        let entry = entry.clone();
        remove.connect_clicked(move |_| {
            let Some(editor) = dialog.upgrade() else {
                return;
            };
            let delete = delete.clone();
            let closing = editor.downgrade();
            confirm_delete(&editor, &entry, hours_format, now, move || {
                delete();
                if let Some(editor) = closing.upgrade() {
                    editor.force_close();
                }
            });
        });
    }
    {
        let entry = entry.clone();
        dialog.connect_response(Some("save"), move |_, _| {
            save(Changes::of(
                &entry,
                chooser.selected(),
                &notes.text(),
                &time.text(),
                &opened,
                format::decimal_separator(),
            ));
        });
    }
    dialog.present(Some(parent));
}

/// Logs time on `day` without a timer: a project and task, a length and notes.
/// `log` gets them once Log is pressed, which it can be only with a length.
pub fn present_logger(
    parent: &impl IsA<gtk::Widget>,
    day_label: &str,
    projects: Vec<ProjectTargets>,
    log: impl Fn(TimerTarget, f64, Option<String>) + 'static,
) {
    let dialog = adw::AlertDialog::new(
        Some("Log time"),
        Some(&format!("{day_label}, with no timer")),
    );
    dialog.add_responses(&[("cancel", "Cancel"), ("log", "Log")]);
    dialog.set_response_appearance("log", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("log"));
    dialog.set_close_response("cancel");
    dialog.set_response_enabled("log", false);

    let chooser = Chooser::new(projects, None);
    let time = adw::EntryRow::builder()
        .title("Time, such as 1:30 or 1.5")
        .activates_default(true)
        .build();
    let notes = adw::EntryRow::builder().title("Notes").build();
    let fields = gtk::ListBox::new();
    fields.add_css_class("boxed-list");
    fields.set_selection_mode(gtk::SelectionMode::None);
    fields.append(&chooser.project);
    fields.append(&chooser.task);
    fields.append(&time);
    fields.append(&notes);
    dialog.set_extra_child(Some(&fields));

    {
        let dialog = dialog.downgrade();
        let has_target = chooser.selected().is_some();
        time.connect_changed(move |row| {
            let loggable = logged_hours(&row.text()).is_some();
            if loggable || row.text().is_empty() {
                row.remove_css_class("error");
            } else {
                row.add_css_class("error");
            }
            if let Some(dialog) = dialog.upgrade() {
                dialog.set_response_enabled("log", loggable && has_target);
            }
        });
    }
    dialog.connect_response(Some("log"), move |_, _| {
        if let (Some(target), Some(hours)) = (chooser.selected(), logged_hours(&time.text())) {
            let notes = notes.text().trim().to_string();
            log(target, hours, (!notes.is_empty()).then_some(notes));
        }
    });
    dialog.present(Some(parent));
}

/// Selects all of `row`'s text whenever it gains focus, as a Mac text field
/// does, so that typing `1` into a total of `0:00` gives `1`, not `10:00`.
///
/// GTK selects all only when focus arrives from the keyboard. A click places the
/// cursor where it lands, and while the button is down every movement of the
/// pointer, however small, moves that end of the selection with it. So a click
/// that brings focus here selects all when the button comes up, unless the
/// pointer has moved far enough to be a drag, which keeps what it selected.
///
/// A field also keeps its selection when it loses focus, and GTK treats a press
/// inside a selection as the start of a drag: if none follows, the release
/// collapses the selection to where it was clicked. So whatever was selected
/// goes as focus arrives, which is before GTK looks for a selection.
fn select_all_on_focus(row: &adw::EntryRow) {
    use gtk::{gdk, glib};
    use std::cell::Cell;

    // Where the press now bringing focus here began, if one is.
    let press: Rc<Cell<Option<(f64, f64)>>> = Rc::default();
    let focus = gtk::EventControllerFocus::new();

    let pointer = gtk::EventControllerLegacy::new();
    // Before the field's own handling: a press must be seen before the field
    // takes focus, and a release before the field reacts to it.
    pointer.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let target = row.downgrade();
        let press = press.clone();
        let focus = focus.clone();
        pointer.connect_event(move |_, event| {
            let primary = event
                .downcast_ref::<gdk::ButtonEvent>()
                .is_some_and(|button| button.button() == gdk::BUTTON_PRIMARY);
            match (event.event_type(), event.position()) {
                (gdk::EventType::ButtonPress, Some(at)) if primary && !focus.contains_focus() => {
                    press.set(Some(at));
                }
                (gdk::EventType::ButtonRelease, Some((x, y))) if primary => {
                    if let (Some((from_x, from_y)), Some(row)) = (press.take(), target.upgrade())
                        && !row.drag_check_threshold(
                            from_x as i32,
                            from_y as i32,
                            x as i32,
                            y as i32,
                        )
                    {
                        select_all_soon(&row);
                    }
                }
                _ => {}
            }
            glib::Propagation::Proceed
        });
    }
    row.add_controller(pointer);

    // Whether focus left with the window, when another window became active.
    // GTK reports its return as focus arriving, but nothing should change then:
    // the cursor and any selection stay where they were, as on the Mac.
    let away = Rc::new(Cell::new(false));
    let target = row.downgrade();
    {
        let press = press.clone();
        let away = away.clone();
        focus.connect_enter(move |_| {
            if away.replace(false) {
                return;
            }
            if let Some(row) = target.upgrade() {
                row.select_region(0, 0);
                if press.get().is_none() {
                    select_all_soon(&row);
                }
            }
        });
    }
    let target = row.downgrade();
    focus.connect_leave(move |_| {
        // A release this never saw must not stop the next focus from selecting.
        press.set(None);
        let window_inactive = target
            .upgrade()
            .and_then(|row| row.root())
            .and_downcast::<gtk::Window>()
            .is_some_and(|window| !window.is_active());
        away.set(window_inactive);
    });
    row.add_controller(focus);
}

/// Selects all of `row`'s text once the event being handled is done with, and
/// ahead of any input already waiting, so a key pressed by then replaces the
/// text rather than going into it.
fn select_all_soon(row: &adw::EntryRow) {
    use gtk::glib;
    let target = row.downgrade();
    glib::idle_add_local_full(glib::Priority::HIGH, move || {
        if let Some(row) = target.upgrade() {
            row.select_region(0, -1);
        }
        glib::ControlFlow::Break
    });
}

/// The hours in `text`, if it is a length worth logging.
fn logged_hours(text: &str) -> Option<f64> {
    parse_hours(text, format::decimal_separator()).filter(|hours| *hours > 0.0)
}

/// Asks before deleting: Harvest has no undo.
pub fn confirm_delete(
    parent: &impl IsA<gtk::Widget>,
    entry: &TrackedEntry,
    hours_format: HoursFormat,
    now: Timestamp,
    delete: impl Fn() + 'static,
) {
    let ask = adw::AlertDialog::new(
        Some("Delete this time entry?"),
        Some(&format!(
            "{} on {} will be removed from Harvest. This cannot be undone.",
            format::hours(entry.hours(now), hours_format),
            entry.task.name
        )),
    );
    ask.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
    ask.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    ask.set_close_response("cancel");
    ask.connect_response(Some("delete"), move |_, _| delete());
    ask.present(Some(parent));
}

/// Play on an entry from a day that is over usually means "do that work now",
/// not "add to that day", so it asks, as the Mac and Harvest's own app do. Neither
/// choice is a cancel: one times the same project and task today, in today's
/// entry for it if there is one; the other reopens the old day.
pub fn confirm_resume(
    parent: &impl IsA<gtk::Widget>,
    entry: &TrackedEntry,
    start_today: impl Fn() + 'static,
    resume: impl Fn() + 'static,
) {
    let day = format::day(entry.spent_date);
    let ask = adw::AlertDialog::new(
        Some(&format!("This entry is from {day}.")),
        Some(&format!("Starting on Today leaves {day}’s time as it is.")),
    );
    ask.add_responses(&[
        ("cancel", "Cancel"),
        ("resume", &format!("Resume on {day}")),
        ("today", "Start on Today"),
    ]);
    ask.set_response_appearance("today", adw::ResponseAppearance::Suggested);
    ask.set_default_response(Some("today"));
    ask.set_close_response("cancel");
    ask.connect_response(Some("today"), move |_, _| start_today());
    ask.connect_response(Some("resume"), move |_, _| resume());
    ask.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheaves_core::calendar_date::CalendarDate;
    use sheaves_core::models::Reference;
    use sheaves_core::tracked_entry::EntryId;

    fn entry(notes: Option<&str>, running: bool) -> TrackedEntry {
        let at: Timestamp = "2026-11-04T09:00:00Z".parse().unwrap();
        TrackedEntry {
            id: EntryId::Server(1),
            project: Reference::new(1, "Online Store"),
            task: Reference::new(2, "Programming"),
            client: None,
            notes: notes.map(str::to_string),
            spent_date: CalendarDate::new(2026, 11, 4),
            banked_hours: 1.5,
            is_running: running,
            timer_started_at: running.then_some(at),
            is_locked: false,
            is_billable: true,
            is_pending: false,
            updated_at: at,
        }
    }

    #[test]
    fn nothing_changed_asks_for_nothing() {
        let changes = Changes::of(
            &entry(Some("Checkout"), false),
            None,
            " Checkout ",
            "1:30",
            "1:30",
            '.',
        );
        assert_eq!(changes, Changes::default());
    }

    #[test]
    fn new_notes_and_a_new_total_are_sent() {
        let changes = Changes::of(
            &entry(None, false),
            None,
            "Checkout flow",
            "2:15",
            "1:30",
            '.',
        );
        assert_eq!(
            changes,
            Changes {
                target: None,
                notes: Some("Checkout flow".to_string()),
                hours: Some(2.25),
            }
        );
    }

    /// The core's rule: a running timer's field left as it opened sends nothing,
    /// or the seconds since the minute shown would be lost.
    #[test]
    fn a_running_total_left_alone_is_not_sent() {
        let changes = Changes::of(&entry(None, true), None, "", "2:07", "2:07", '.');
        assert_eq!(changes.hours, None);
    }

    #[test]
    fn something_that_is_not_a_duration_changes_nothing() {
        let changes = Changes::of(&entry(None, false), None, "", "soon", "1:30", '.');
        assert_eq!(changes.hours, None);
    }

    /// Saving with the project and task it already has asks for no move.
    #[test]
    fn the_same_project_and_task_is_no_move() {
        let entry = entry(None, false);
        let same = Changes::of(&entry, Some(entry.target()), "", "1:30", "1:30", '.');
        assert_eq!(same.target, None);
        let mut other = entry.target();
        other.task = Reference::new(3, "Design");
        let moved = Changes::of(&entry, Some(other.clone()), "", "1:30", "1:30", '.');
        assert_eq!(moved.target, Some(other));
    }

    #[test]
    fn only_a_length_is_logged() {
        assert_eq!(logged_hours("1:30"), Some(1.5));
        assert_eq!(logged_hours("0"), None);
        assert_eq!(logged_hours("soon"), None);
    }

    #[test]
    fn clearing_the_notes_is_a_change() {
        let changes = Changes::of(
            &entry(Some("Checkout"), false),
            None,
            "",
            "1:30",
            "1:30",
            '.',
        );
        assert_eq!(changes.notes, Some(String::new()));
    }
}
