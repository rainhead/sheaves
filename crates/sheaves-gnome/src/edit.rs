//! Changing an entry: its notes and its total, deleting it, and resuming one from
//! a day that is over. The questions are the Mac's (`Apps/Mac/Sources/EntryRow.swift`),
//! in its words; the rules are the core's.

use std::rc::Rc;

use adw::prelude::*;
use jiff::Timestamp;
use sheaves_core::hours::{HoursEdit, HoursFormat};
use sheaves_core::tracked_entry::TrackedEntry;

use crate::format;

/// What the editor asks the tracker to change. Each is `None` when it was left as
/// it was, or typed as something that is not a duration.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Changes {
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
        notes: &str,
        hours: &str,
        hours_as_opened: &str,
        decimal_separator: char,
    ) -> Self {
        let notes = notes.trim();
        Self {
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
    let fields = gtk::ListBox::new();
    fields.add_css_class("boxed-list");
    fields.set_selection_mode(gtk::SelectionMode::None);
    fields.append(&notes);
    fields.append(&time);

    let remove = gtk::Button::with_label("Delete…");
    remove.add_css_class("destructive-action");
    remove.set_halign(gtk::Align::Start);
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.append(&fields);
    column.append(&remove);
    dialog.set_extra_child(Some(&column));

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
                &notes.text(),
                &time.text(),
                &opened,
                format::decimal_separator(),
            ));
        });
    }
    dialog.present(Some(parent));
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
/// choice is a cancel: one starts a fresh timer today with the same project, task
/// and notes; the other reopens the old day.
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
            " Checkout ",
            "1:30",
            "1:30",
            '.',
        );
        assert_eq!(changes, Changes::default());
    }

    #[test]
    fn new_notes_and_a_new_total_are_sent() {
        let changes = Changes::of(&entry(None, false), "Checkout flow", "2:15", "1:30", '.');
        assert_eq!(
            changes,
            Changes {
                notes: Some("Checkout flow".to_string()),
                hours: Some(2.25),
            }
        );
    }

    /// The core's rule: a running timer's field left as it opened sends nothing,
    /// or the seconds since the minute shown would be lost.
    #[test]
    fn a_running_total_left_alone_is_not_sent() {
        let changes = Changes::of(&entry(None, true), "", "2:07", "2:07", '.');
        assert_eq!(changes.hours, None);
    }

    #[test]
    fn something_that_is_not_a_duration_changes_nothing() {
        let changes = Changes::of(&entry(None, false), "", "soon", "1:30", '.');
        assert_eq!(changes.hours, None);
    }

    #[test]
    fn clearing_the_notes_is_a_change() {
        let changes = Changes::of(&entry(Some("Checkout"), false), "", "1:30", "1:30", '.');
        assert_eq!(changes.notes, Some(String::new()));
    }
}
