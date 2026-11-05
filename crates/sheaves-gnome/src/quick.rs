//! Quick entry: a small window that starts a timer from anywhere, as the Mac's
//! ⌃⌥⌘T panel does. `sheaves-gnome --quick-entry` raises it in the instance
//! already running, and a key in GNOME's custom shortcuts runs that (see
//! [`crate::shortcut`]).
//!
//! What is running sits at the top with a Stop button, or else how much is
//! logged today. Beneath, the projects to start a timer on, as the window ranks
//! them: typing filters them, the arrow keys move between them, ⏎ starts the one
//! chosen, and Escape closes the window.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};
use sheaves_core::ffi::TrackerSnapshot;

use crate::format;
use crate::projects::ProjectList;
use crate::window::{self, Action};

pub struct QuickEntry {
    window: adw::Window,
    search: gtk::SearchEntry,
    projects: Rc<ProjectList>,
    scrolled: gtk::ScrolledWindow,
    task: gtk::Label,
    project: gtk::Label,
    hours: gtk::Label,
    running: gtk::Box,
    idle: gtk::Label,
    status: gtk::Label,
    actions: async_channel::Sender<Action>,
    latest: std::cell::RefCell<Option<TrackerSnapshot>>,
}

impl QuickEntry {
    pub fn new(app: &adw::Application, actions: async_channel::Sender<Action>) -> Rc<Self> {
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Find a project or task"));

        let window = adw::Window::builder()
            .application(app)
            .title("Start a timer")
            .default_width(480)
            .resizable(false)
            .hide_on_close(true)
            .build();

        let projects = {
            let actions = actions.clone();
            let window = window.downgrade();
            ProjectList::new(&search, move |target| {
                let _ = actions.try_send(Action::Start(target));
                if let Some(window) = window.upgrade() {
                    window.set_visible(false);
                }
            })
        };
        projects.make_choosable();

        let task = gtk::Label::new(None);
        task.add_css_class("heading");
        task.set_xalign(0.0);
        task.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let project = gtk::Label::new(None);
        project.add_css_class("caption");
        project.add_css_class("dim-label");
        project.set_xalign(0.0);
        project.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let named = gtk::Box::new(gtk::Orientation::Vertical, 0);
        named.set_hexpand(true);
        named.append(&task);
        named.append(&project);
        let hours = gtk::Label::new(None);
        hours.add_css_class("numeric");
        let stop = gtk::Button::with_label("Stop");
        stop.set_valign(gtk::Align::Center);
        let running = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        running.append(&named);
        running.append(&hours);
        running.append(&stop);
        let idle = gtk::Label::new(None);
        idle.add_css_class("dim-label");
        idle.set_xalign(0.0);

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(320)
            .child(&projects.list)
            .build();

        let status = gtk::Label::new(None);
        status.add_css_class("caption");
        status.add_css_class("dim-label");
        status.set_xalign(0.0);

        let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
        column.set_margin_top(12);
        column.set_margin_bottom(12);
        column.set_margin_start(12);
        column.set_margin_end(12);
        column.append(&running);
        column.append(&idle);
        column.append(&search);
        column.append(&scrolled);
        column.append(&status);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&column));
        window.set_content(Some(&toolbar));

        let quick = Rc::new(Self {
            window,
            search,
            projects,
            scrolled,
            task,
            project,
            hours,
            running,
            idle,
            status,
            actions,
            latest: std::cell::RefCell::default(),
        });
        quick.connect(&stop);
        quick
    }

    fn connect(self: &Rc<Self>, stop: &gtk::Button) {
        {
            let quick = Rc::downgrade(self);
            stop.connect_clicked(move |_| {
                let Some(quick) = quick.upgrade() else { return };
                let running = quick
                    .latest
                    .borrow()
                    .as_ref()
                    .and_then(|snapshot| snapshot.running_entry.clone());
                if let Some(entry) = running {
                    let _ = quick.actions.try_send(Action::Toggle(entry));
                }
                quick.window.set_visible(false);
            });
        }
        {
            // The first match chosen as the search changes, so ⏎ starts what is
            // at the top of what is showing.
            let quick = Rc::downgrade(self);
            self.search.connect_search_changed(move |_| {
                if let Some(quick) = quick.upgrade() {
                    quick.choose_first();
                }
            });
        }
        {
            let quick = Rc::downgrade(self);
            self.search.connect_activate(move |_| {
                if let Some(quick) = quick.upgrade() {
                    quick.start_chosen();
                }
            });
        }
        {
            let quick = Rc::downgrade(self);
            self.projects.list.connect_row_activated(move |_, row| {
                if let Some(quick) = quick.upgrade() {
                    quick.projects.start(row);
                }
            });
        }
        // Captured before the search field sees them: the arrows move through the
        // list while typing goes on in the field, and Escape closes from anywhere.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let quick = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(quick) = quick.upgrade() else {
                return glib::Propagation::Proceed;
            };
            match key {
                gdk::Key::Down => quick.move_by(1),
                gdk::Key::Up => quick.move_by(-1),
                gdk::Key::Escape => {
                    quick.window.set_visible(false);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        self.window.add_controller(keys);
    }

    /// Opens fresh: nothing typed, the top project chosen, and the cursor in the
    /// search field. It also looks at Harvest, as the Mac's panel does.
    pub fn present(&self) {
        self.search.set_text("");
        self.choose_first();
        self.window.present();
        self.search.grab_focus();
        let _ = self.actions.try_send(Action::Sync);
    }

    pub fn render(&self, snapshot: &TrackerSnapshot) {
        let format = window::hours_format(snapshot);
        match &snapshot.running_entry {
            Some(entry) => {
                self.task.set_label(&entry.task.name);
                self.project.set_label(&entry.target().project_label());
                self.hours
                    .set_label(&format::hours(entry.hours(snapshot.state.now), format));
                self.running.set_visible(true);
                self.idle.set_visible(false);
            }
            None => {
                self.idle.set_label(&format!(
                    "Nothing running · {} today",
                    format::hours(snapshot.total_hours, format)
                ));
                self.running.set_visible(false);
                self.idle.set_visible(true);
            }
        }
        self.status.set_label(&window::status_text(snapshot));
        let changed = self.projects.show(&snapshot.ordered_projects);
        if changed {
            self.choose_first();
        }
        *self.latest.borrow_mut() = Some(snapshot.clone());
    }

    fn choose_first(&self) {
        let first = self.projects.visible_rows().into_iter().next();
        self.projects.list.select_row(first.as_ref());
    }

    fn move_by(&self, offset: i32) -> glib::Propagation {
        let rows = self.projects.visible_rows();
        if rows.is_empty() {
            return glib::Propagation::Proceed;
        }
        let current = self
            .projects
            .list
            .selected_row()
            .and_then(|selected| rows.iter().position(|row| *row == selected))
            .unwrap_or(0) as i32;
        let next = (current + offset).clamp(0, rows.len() as i32 - 1) as usize;
        self.projects.list.select_row(Some(&rows[next]));
        // Scrolled into view without taking the focus, so typing carries on in
        // the search field.
        if let Some(top) =
            rows[next].compute_point(&self.projects.list, &gtk::graphene::Point::new(0.0, 0.0))
        {
            let top = f64::from(top.y());
            self.scrolled
                .vadjustment()
                .clamp_page(top, top + f64::from(rows[next].height()));
        }
        glib::Propagation::Stop
    }

    fn start_chosen(&self) {
        let chosen = self
            .projects
            .list
            .selected_row()
            .filter(|row| row.is_child_visible())
            .or_else(|| self.projects.visible_rows().into_iter().next());
        if let Some(row) = chosen {
            self.projects.start(&row);
        }
    }

    #[cfg(debug_assertions)]
    pub fn window(&self) -> &adw::Window {
        &self.window
    }

    /// Types `text` into the search, for the documentation images.
    #[cfg(debug_assertions)]
    pub fn type_search(&self, text: &str) {
        self.search.set_text(text);
    }
}
