//! The projects a timer can be started on, in the order the core ranks them, each
//! with its tasks to choose from, filtered by what is typed into a search field.
//! The window and the quick entry show the same list.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use crate::budget;
use sheaves_core::ffi::TrackerSnapshot;
use sheaves_core::tracked_entry::{ProjectTargets, TimerTarget};

/// Overspending is worth a colour, as on the Mac. Pango markup cannot name the
/// theme's red, so this is GNOME's own.
const OVER: &str = "#e01b24";

/// The client, and the budget after it when there is one, as markup: "123
/// Industries · 1,007 left", the figure red when it is an overrun.
fn subtitle(client: &str, budget: Option<&(String, bool)>) -> String {
    let client = glib::markup_escape_text(client);
    let Some((text, over)) = budget else {
        return client.to_string();
    };
    let text = glib::markup_escape_text(text);
    let figure = if *over {
        format!("<span foreground=\"{OVER}\">{text}</span>")
    } else {
        text.to_string()
    };
    if client.is_empty() {
        figure
    } else {
        format!("{client} · {figure}")
    }
}

fn client_name(project: &ProjectTargets) -> &str {
    project
        .client
        .as_ref()
        .map_or("", |client| client.name.as_str())
}

pub struct ProjectList {
    pub list: gtk::ListBox,
    /// What each row is searched by, in row order.
    haystacks: Rc<RefCell<Vec<String>>>,
    shown: RefCell<Vec<ProjectTargets>>,
    /// The budget text each shown row has. A change of budget alone rewrites the
    /// rows' subtitles in place, so a project chosen in quick entry stays chosen.
    shown_budgets: RefCell<Vec<Option<(String, bool)>>>,
    /// The rows, in order, for rewriting their subtitles.
    rows: RefCell<Vec<adw::ActionRow>>,
    /// Each row's task menu, in row order.
    tasks: RefCell<Vec<gtk::DropDown>>,
    start: Rc<dyn Fn(TimerTarget)>,
    /// Whether a row can be chosen and clicked to start, as in quick entry.
    choosable: std::cell::Cell<bool>,
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
            shown_budgets: RefCell::default(),
            rows: RefCell::default(),
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
    pub fn show(&self, snapshot: &TrackerSnapshot) -> bool {
        let projects = &snapshot.ordered_projects;
        let format = crate::window::hours_format(snapshot);
        let budgets: Vec<Option<(String, bool)>> = projects
            .iter()
            .map(|project| {
                let budget = snapshot.state.budgets.get(&project.project.id)?;
                budget::left(budget, format).map(|left| (left.text, left.over))
            })
            .collect();
        if *self.shown.borrow() == projects.as_slice() {
            if *self.shown_budgets.borrow() != budgets {
                for ((row, project), budget) in
                    self.rows.borrow().iter().zip(projects).zip(&budgets)
                {
                    row.set_subtitle(&subtitle(client_name(project), budget.as_ref()));
                }
                *self.shown_budgets.borrow_mut() = budgets;
            }
            return false;
        }
        self.list.remove_all();
        let mut haystacks = Vec::new();
        let mut menus = Vec::new();
        let mut rows = Vec::new();
        for (project, budget) in projects.iter().zip(&budgets) {
            haystacks.push(haystack(project));
            let row = adw::ActionRow::builder()
                .activatable(self.choosable.get())
                .title(glib::markup_escape_text(&project.project.name))
                .subtitle(subtitle(client_name(project), budget.as_ref()))
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
            rows.push(row);
            menus.push(tasks);
        }
        *self.haystacks.borrow_mut() = haystacks;
        *self.tasks.borrow_mut() = menus;
        *self.rows.borrow_mut() = rows;
        *self.shown.borrow_mut() = projects.to_vec();
        *self.shown_budgets.borrow_mut() = budgets;
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
    fn budget_follows_the_client() {
        assert_eq!(subtitle("A & B", None), "A &amp; B");
        let left = ("8:00 left".to_string(), false);
        assert_eq!(subtitle("ABC", Some(&left)), "ABC · 8:00 left");
        assert_eq!(subtitle("", Some(&left)), "8:00 left");
        let over = ("$240 over".to_string(), true);
        assert_eq!(
            subtitle("ABC", Some(&over)),
            format!("ABC · <span foreground=\"{OVER}\">$240 over</span>")
        );
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
