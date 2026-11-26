//! The projects a timer can be started on, in the order the core ranks them, each
//! with its tasks to choose from, filtered by what is typed into a search field.
//! The window and the quick entry show the same list.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use sheaves_core::tracked_entry::{ProjectTargets, TimerTarget};

pub struct ProjectList {
    pub list: gtk::ListBox,
    shown: Rc<RefCell<Vec<ProjectTargets>>>,
    /// Each row's task menu, in row order.
    tasks: RefCell<Vec<gtk::DropDown>>,
    start: Rc<dyn Fn(TimerTarget)>,
    /// Whether a row can be chosen and clicked to start, as in quick entry.
    choosable: std::cell::Cell<bool>,
}

impl ProjectList {
    /// A list filtered by `search`, which calls `start` with the project and
    /// task a row's start button asks for.
    pub fn new(search: &gtk::SearchEntry, start: impl Fn(TimerTarget) + 'static) -> Rc<Self> {
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let shown: Rc<RefCell<Vec<ProjectTargets>>> = Rc::default();
        {
            let shown = shown.clone();
            let search = search.clone();
            list.set_filter_func(move |row| {
                shown
                    .borrow()
                    .get(row.index() as usize)
                    .is_none_or(|project| project.matches_query(&search.text()))
            });
        }
        {
            let list = list.clone();
            search.connect_search_changed(move |_| list.invalidate_filter());
        }
        Rc::new(Self {
            list,
            shown,
            tasks: RefCell::default(),
            start: Rc::new(start),
            choosable: std::cell::Cell::new(false),
        })
    }

    /// Lets one row at a time be chosen, and a click on a row start it. Action
    /// rows cannot be activated unless told so, and a click would only choose.
    pub fn make_choosable(&self) {
        self.choosable.set(true);
        self.list.set_selection_mode(gtk::SelectionMode::Single);
        let mut index = 0;
        while let Some(row) = self.list.row_at_index(index) {
            row.set_activatable(true);
            index += 1;
        }
    }

    /// Shows `projects`, rebuilding the rows only when they changed, so a row is
    /// not swapped out from under the pointer once a second. Returns whether it
    /// rebuilt them.
    pub fn show(&self, projects: &[ProjectTargets]) -> bool {
        if *self.shown.borrow() == projects {
            return false;
        }
        self.list.remove_all();
        let mut menus = Vec::new();
        for project in projects {
            let row = adw::ActionRow::builder()
                .activatable(self.choosable.get())
                .title(glib::markup_escape_text(&project.project.name))
                .subtitle(glib::markup_escape_text(
                    project
                        .client
                        .as_ref()
                        .map_or("", |client| client.name.as_str()),
                ))
                .build();
            let names: Vec<&str> = project
                .tasks
                .iter()
                .map(|task| task.name.as_str())
                .collect();
            let tasks = gtk::DropDown::from_strings(&names);
            tasks.set_valign(gtk::Align::Center);
            let start = gtk::Button::from_icon_name("media-playback-start-symbolic");
            start.set_tooltip_text(Some("Start a timer"));
            start.add_css_class("flat");
            start.set_valign(gtk::Align::Center);
            let starting = self.start.clone();
            let project = project.clone();
            let chosen = tasks.clone();
            start.connect_clicked(move |_| {
                if let Some(task) = project.tasks.get(chosen.selected() as usize) {
                    starting(project.target(task.clone()));
                }
            });
            row.add_suffix(&tasks);
            row.add_suffix(&start);
            self.list.append(&row);
            menus.push(tasks);
        }
        *self.tasks.borrow_mut() = menus;
        *self.shown.borrow_mut() = projects.to_vec();
        self.list.invalidate_filter();
        true
    }

    /// The project and task `row` would start, with the task its menu shows.
    pub fn target_at(&self, row: &gtk::ListBoxRow) -> Option<TimerTarget> {
        let index = row.index() as usize;
        let project = self.shown.borrow().get(index)?.clone();
        let selected = self.tasks.borrow().get(index)?.selected() as usize;
        let task = project.tasks.get(selected)?.clone();
        Some(project.target(task))
    }

    /// The rows the search leaves showing, top first.
    pub fn visible_rows(&self) -> Vec<gtk::ListBoxRow> {
        let mut rows = Vec::new();
        let mut index = 0;
        while let Some(row) = self.list.row_at_index(index) {
            if row.is_child_visible() {
                rows.push(row);
            }
            index += 1;
        }
        rows
    }

    /// Starts the timer `row` offers.
    pub fn start(&self, row: &gtk::ListBoxRow) {
        if let Some(target) = self.target_at(row) {
            (self.start)(target);
        }
    }
}
