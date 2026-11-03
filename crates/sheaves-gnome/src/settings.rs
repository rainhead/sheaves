//! Settings: the Harvest connection, starting at login, and which core this is.

use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::glib;
use sheaves_core::client::Credentials;
use sheaves_core::ffi::{ConnectFailure, Tracker, TrackerSnapshot};
use sheaves_core::tracker::Connection;

use crate::autostart;

/// Builds and shows Settings over `parent`, for the state in `snapshot`, with the
/// peers named now and where to send them when they change.
pub fn present(
    parent: &adw::ApplicationWindow,
    tracker: Arc<Tracker>,
    snapshot: &TrackerSnapshot,
    peers: Vec<String>,
    set_peers: Rc<dyn Fn(Vec<String>)>,
) {
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("Settings");
    let page = adw::PreferencesPage::new();
    page.add(&harvest_group(&dialog, tracker.clone(), snapshot));
    page.add(&startup_group());
    page.add(&peers_group(tracker, peers, set_peers));
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
        group.add(&account);
        group.add(&user);

        let disconnect = gtk::Button::with_label("Disconnect…");
        disconnect.add_css_class("destructive-action");
        disconnect.set_halign(gtk::Align::End);
        disconnect.set_margin_top(12);
        let dialog = dialog.clone();
        disconnect.connect_clicked(move |_| {
            let confirm = adw::AlertDialog::new(
                Some("Disconnect from Harvest?"),
                Some("The token is removed from the keyring, and anything not yet sent to Harvest is discarded."),
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
        "Create a personal access token at id.getharvest.com/developers, and paste it here with the account ID shown beside it. The token is kept in your keyring.",
    ));
    let account = adw::EntryRow::builder().title("Account ID").build();
    let token = adw::PasswordEntryRow::builder().title("Token").build();
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
    group.add(&connect);

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
                            Some("Discard unsent changes?"),
                            Some(&format!(
                                "{count} change{} made under a different Harvest account {} never sent. Connecting with this token discards {}.",
                                if count == 1 { "" } else { "s" },
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

fn peers_group(
    tracker: Arc<Tracker>,
    peers: Vec<String>,
    set_peers: Rc<dyn Fn(Vec<String>)>,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title("Peers");
    group.set_description(Some(
        "Other machines running Sheaves, by their names on your tailnet. Before asking whether you were away, this machine asks them whether they saw you meanwhile, and it answers them in turn. Only machines on the tailnet are asked or answered.",
    ));
    let names = adw::EntryRow::builder()
        .title("Peers")
        .text(peers.join(", "))
        .build();
    // Read as typed, so a separator typed last is not taken away mid-word.
    names.connect_changed(move |row| set_peers(crate::config::names_in(&row.text())));
    let answering = adw::ActionRow::builder().title("Answering on").build();
    let addresses = dim("");
    answering.add_suffix(&addresses);
    group.add(&names);
    group.add(&answering);

    // The core binds in the background and looks for new tailnet addresses once a
    // minute, so this is read again while Settings is open.
    let refresh = move || {
        let listening = tracker.listening_on();
        addresses.set_label(&if listening.is_empty() {
            "Nowhere: name a peer, and check Tailscale is running".to_string()
        } else {
            listening.join(", ")
        });
    };
    refresh();
    let weak = group.downgrade();
    glib::timeout_add_seconds_local(3, move || {
        if weak.upgrade().is_none() {
            return glib::ControlFlow::Break;
        }
        refresh();
        glib::ControlFlow::Continue
    });
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
