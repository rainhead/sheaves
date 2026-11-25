//! Settings: the Harvest connection, starting at login, and which core this is.

use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::glib;
use sheaves_core::client::Credentials;
use sheaves_core::ffi::{ConnectFailure, Tracker, TrackerSnapshot};
use sheaves_core::tracker::Connection;

use crate::autostart;

/// Builds and shows Settings over `parent`, for the state in `snapshot`.
pub fn present(parent: &adw::ApplicationWindow, tracker: Arc<Tracker>, snapshot: &TrackerSnapshot) {
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("Settings");
    let page = adw::PreferencesPage::new();
    page.add(&harvest_group(&dialog, tracker, snapshot));
    page.add(&shortcut_group());
    page.add(&startup_group());
    page.add(&about_group());
    dialog.add(&page);
    dialog.present(Some(parent));
}

fn harvest_group(
    dialog: &adw::PreferencesDialog,
    tracker: Arc<Tracker>,
    snapshot: &TrackerSnapshot,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title("Harvest");
    let state = &snapshot.state;

    if state.connection != Connection::NeedsCredentials {
        let account = adw::ActionRow::builder().title("Account").build();
        account.add_suffix(&dim(state
            .company
            .as_ref()
            .map_or("—", |c| c.name.as_str())));
        let user = adw::ActionRow::builder().title("Signed in as").build();
        user.add_suffix(&dim(&state
            .user
            .as_ref()
            .map_or("—".to_string(), |u| u.name())));
        let projects = adw::ActionRow::builder().title("Projects").build();
        projects.add_suffix(&dim(&format!("{} task assignments", state.targets.len())));
        group.add(&account);
        group.add(&user);
        group.add(&projects);

        let disconnect = gtk::Button::with_label("Disconnect…");
        disconnect.add_css_class("destructive-action");
        disconnect.set_halign(gtk::Align::End);
        disconnect.set_margin_top(12);
        let dialog = dialog.clone();
        let pending = state.pending_count;
        disconnect.connect_clicked(move |_| {
            let confirm = adw::AlertDialog::new(
                Some("Disconnect from Harvest?"),
                Some(&if pending > 0 {
                    format!(
                        "{pending} change{} not reached Harvest yet and will be discarded along with the token.",
                        if pending == 1 { " has" } else { "s have" },
                    )
                } else {
                    "The token will be removed from your keyring. Cached entries are cleared."
                        .to_string()
                }),
            );
            confirm.add_responses(&[("cancel", "Cancel"), ("disconnect", "Disconnect")]);
            confirm.set_response_appearance("disconnect", adw::ResponseAppearance::Destructive);
            let tracker = tracker.clone();
            let settings = dialog.clone();
            confirm.connect_response(Some("disconnect"), move |_, _| {
                let tracker = tracker.clone();
                glib::spawn_future_local(async move { tracker.disconnect().await });
                settings.close();
            });
            confirm.present(Some(&dialog));
        });
        group.add(&disconnect);
        return group;
    }

    group.set_description(Some(
        "Create a personal access token at id.getharvest.com/developers. The page shows the token and the account IDs it can reach. Sheaves stores the token in your keyring and sends it only to api.harvestapp.com.",
    ));
    let account = adw::EntryRow::builder().title("Account ID").build();
    let token = adw::PasswordEntryRow::builder()
        .title("Personal Access Token")
        .build();
    let error = gtk::Label::new(None);
    error.add_css_class("error");
    error.set_wrap(true);
    error.set_xalign(0.0);
    error.set_visible(false);
    let connect = gtk::Button::with_label("Connect");
    connect.add_css_class("suggested-action");
    connect.set_halign(gtk::Align::End);
    connect.set_margin_top(12);
    group.add(&account);
    group.add(&token);
    group.add(&error);
    let create =
        gtk::LinkButton::with_label("https://id.getharvest.com/developers", "Create a token…");
    create.set_halign(gtk::Align::Start);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    buttons.set_margin_top(12);
    connect.set_margin_top(0);
    connect.set_hexpand(true);
    connect.set_halign(gtk::Align::End);
    buttons.append(&create);
    buttons.append(&connect);
    group.add(&buttons);

    let attempt = Rc::new({
        let dialog = dialog.clone();
        let error = error.clone();
        let connect_button = connect.clone();
        move |discarding: bool| {
            let credentials = Credentials::new(account.text(), token.text());
            let tracker = tracker.clone();
            let dialog = dialog.clone();
            let error = error.clone();
            let button = connect_button.clone();
            button.set_sensitive(false);
            glib::spawn_future_local(async move {
                let result = tracker.connect(credentials.clone(), discarding).await;
                button.set_sensitive(true);
                match result {
                    Ok(()) => {
                        dialog.close();
                    }
                    Err(ConnectFailure::UnsentChangesBelongElsewhere { count }) => {
                        let ask = adw::AlertDialog::new(
                            Some("Discard changes made under another account?"),
                            Some(&format!(
                                "{count} change{} not reached Harvest yet, and {} made under a different account or user than this token belongs to. To send {}, cancel and connect with a token for that account instead.",
                                if count == 1 { " has" } else { "s have" },
                                if count == 1 { "was" } else { "were" },
                                if count == 1 { "it" } else { "them" },
                            )),
                        );
                        ask.add_responses(&[
                            ("cancel", "Cancel"),
                            ("discard", "Discard and Connect"),
                        ]);
                        ask.set_response_appearance(
                            "discard",
                            adw::ResponseAppearance::Destructive,
                        );
                        let tracker = tracker.clone();
                        let settings = dialog.clone();
                        let error = error.clone();
                        ask.connect_response(Some("discard"), move |_, _| {
                            let tracker = tracker.clone();
                            let settings = settings.clone();
                            let error = error.clone();
                            let credentials = credentials.clone();
                            glib::spawn_future_local(async move {
                                match tracker.connect(credentials, true).await {
                                    Ok(()) => {
                                        settings.close();
                                    }
                                    Err(failure) => {
                                        error.set_label(&failure.to_string());
                                        error.set_visible(true);
                                    }
                                }
                            });
                        });
                        ask.present(Some(&dialog));
                    }
                    Err(failure) => {
                        error.set_label(&failure.to_string());
                        error.set_visible(true);
                    }
                }
            });
        }
    });
    let on_click = attempt.clone();
    connect.connect_clicked(move |_| on_click(false));
    group
}

/// The key for quick entry, as one of GNOME's custom shortcuts. Switched on, it
/// is Ctrl+Alt+Super+T, which the user can change in GNOME's own Settings. The
/// row says the same thing either way, naming the key it adds or the key that is
/// bound: a description that changed with the switch also changed the row's
/// height.
fn shortcut_group() -> adw::PreferencesGroup {
    use crate::shortcut::{self, Shortcuts};

    let group = adw::PreferencesGroup::new();
    group.set_title("Shortcut");
    group.set_description(Some(
        "To use another key, change it in GNOME's Settings › Keyboard › Custom Shortcuts.",
    ));
    let row = adw::SwitchRow::builder().title("Quick entry").build();
    let error = gtk::Label::new(None);
    error.add_css_class("error");
    error.set_xalign(0.0);
    error.set_wrap(true);
    error.set_visible(false);
    group.add(&row);
    group.add(&error);

    let Some(shortcuts) = Shortcuts::desktop() else {
        row.set_subtitle("GNOME's keyboard shortcuts are not available here.");
        row.set_sensitive(false);
        return group;
    };
    let describe = |row: &adw::SwitchRow, bound: Option<&str>| {
        row.set_subtitle(&format!(
            "{} opens a window to start a timer from any app.",
            shortcut::readable(bound.unwrap_or(shortcut::SUGGESTED))
        ));
    };
    let bound = shortcuts.bound();
    row.set_active(bound.is_some());
    describe(&row, bound.as_deref());

    // As with the login item: putting the switch back after a failure notifies
    // again, and must not undo what it reports.
    let reverting = std::rc::Rc::new(std::cell::Cell::new(false));
    row.connect_active_notify(move |row| {
        if reverting.get() {
            return;
        }
        let result = if row.is_active() {
            autostart::this_program()
                .map_err(|error| error.to_string())
                .and_then(|program| {
                    shortcuts
                        .bind(shortcut::SUGGESTED, &program)
                        .map_err(|error| error.to_string())
                })
        } else {
            shortcuts.unbind().map_err(|error| error.to_string())
        };
        let bound = shortcuts.bound();
        match result {
            Ok(()) => error.set_visible(false),
            Err(failure) => {
                error.set_label(&format!("Could not change the shortcut: {failure}"));
                error.set_visible(true);
                reverting.set(true);
                row.set_active(bound.is_some());
                reverting.set(false);
            }
        }
        describe(row, bound.as_deref());
    });
    group
}

fn startup_group() -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title("Startup");
    let folder = autostart::folder();
    let row = adw::SwitchRow::builder()
        .title("Open Sheaves at login")
        .subtitle("Starts in the top bar, with no window once connected.")
        .active(autostart::is_enabled(&folder))
        .build();
    let error = gtk::Label::new(None);
    error.add_css_class("error");
    error.set_xalign(0.0);
    error.set_visible(false);
    {
        let error = error.clone();
        // Putting the switch back after a failure notifies again, and handled, that
        // second notification would undo the first: with nothing to remove, it hides
        // the error just shown.
        let reverting = std::rc::Rc::new(std::cell::Cell::new(false));
        row.connect_active_notify(move |row| {
            if reverting.get() {
                return;
            }
            let result = if row.is_active() {
                autostart::this_program().and_then(|program| autostart::enable(&folder, &program))
            } else {
                autostart::disable(&folder)
            };
            match result {
                Ok(()) => error.set_visible(false),
                Err(failure) => {
                    error.set_label(&format!("Could not change the login item: {failure}"));
                    error.set_visible(true);
                    reverting.set(true);
                    row.set_active(autostart::is_enabled(&folder));
                    reverting.set(false);
                }
            }
        });
    }
    group.add(&row);
    group.add(&error);
    group
}

fn about_group() -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title("About");
    let version = adw::ActionRow::builder().title("Version").build();
    version.add_suffix(&dim(env!("CARGO_PKG_VERSION")));
    let core = adw::ActionRow::builder().title("Core").build();
    core.add_suffix(&dim(&sheaves_core::core_version()));
    group.add(&version);
    group.add(&core);
    group
}

fn dim(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("dim-label");
    label
}
