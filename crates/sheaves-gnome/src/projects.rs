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
    /// What each row is searched by, in row order.
    haystacks: Rc<RefCell<Vec<String>>>,
    shown: RefCell<Vec<ProjectTargets>>,
    /// Each row's task menu, in row order.
    tasks: RefCell<Vec<gtk::DropDown>>,
    start: Rc<dyn Fn(TimerTarget)>,
}

/// Every word typed appears somewhere in the project's name, its client's or one
/// of its tasks'.
fn matches(haystack: &str, query: &str) -> bool {
    let query = query.to_lowercase();
    query.split_whitespace().all(|word| haystack.contains(word))
}

fn haystack(project: &ProjectTargets) -> String {
    let mut haystack = project.project.name.to_lowercase();
    if let Some(client) = &project.client {
        haystack.push(' ');
        haystack.push_str(&client.name.to_lowercase());
    }
    for task in &project.tasks {
        haystack.push(' ');
        haystack.push_str(&task.name.to_lowercase());
    }
    haystack
}

impl ProjectList {
    /// A list filtered by `search`, which calls `start` with the project and
    /// task a row's start button asks for.
    pub fn new(search: &gtk::SearchEntry, start: impl Fn(TimerTarget) + 'static) -> Rc<Self> {
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let haystacks: Rc<RefCell<Vec<String>>> = Rc::default();
        {
            let haystacks = haystacks.clone();
            let search = search.clone();
            list.set_filter_func(move |row| {
                haystacks
                    .borrow()
                    .get(row.index() as usize)
                    .is_none_or(|haystack| matches(haystack, &search.text()))
            });
        }
        {
            let list = list.clone();
            search.connect_search_changed(move |_| list.invalidate_filter());
        }
        Rc::new(Self {
            list,
            haystacks,
            shown: RefCell::default(),
            tasks: RefCell::default(),
            start: Rc::new(start),
        })
    }

    /// Shows `projects`, rebuilding the rows only when they changed, so a row is
    /// not swapped out from under the pointer once a second. Returns whether it
    /// rebuilt them.
    pub fn show(&self, projects: &[ProjectTargets]) -> bool {
        if *self.shown.borrow() == projects {
            return false;
        }
        self.list.remove_all();
        let mut haystacks = Vec::new();
        let mut menus = Vec::new();
        for project in projects {
            haystacks.push(haystack(project));
            let row = adw::ActionRow::builder()
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
        *self.haystacks.borrow_mut() = haystacks;
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

#[cfg(test)]
mod tests {
    use super::*;
    use sheaves_core::models::Reference;

    fn project(name: &str, client: &str, tasks: &[&str]) -> ProjectTargets {
        ProjectTargets {
            project: Reference::new(1, name),
            client: Some(Reference::new(2, client)),
            tasks: tasks
                .iter()
                .enumerate()
                .map(|(id, task)| Reference::new(id as i64, *task))
                .collect(),
        }
    }

    #[test]
    fn finds_a_project_by_any_of_its_words() {
        let store = haystack(&project("Online Store", "123 Industries", &["Programming"]));
        assert!(matches(&store, ""));
        assert!(matches(&store, "store"));
        assert!(matches(&store, "Industries prog"));
        assert!(!matches(&store, "design"));
    }
}
