//! Choosing a project and one of its tasks: two rows, the second following the
//! first, in the order the core ranks them.

use std::rc::Rc;

use adw::prelude::*;
use sheaves_core::tracked_entry::{ProjectTargets, TimerTarget};

pub struct Chooser {
    pub project: adw::ComboRow,
    pub task: adw::ComboRow,
    projects: Rc<Vec<ProjectTargets>>,
}

/// The client's name, or nothing when the project has none.
fn client(project: &ProjectTargets) -> String {
    project
        .client
        .as_ref()
        .map(|client| client.name.clone())
        .unwrap_or_default()
}

/// The list a project row opens: each project's name, with its client beneath,
/// so two clients' projects of the same name can be told apart. The row itself
/// shows the name alone, with the client as its subtitle: "Client · Project" in
/// the row's value was cut off before the project began.
fn project_list(projects: Rc<Vec<ProjectTargets>>) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let name = gtk::Label::new(None);
        name.set_xalign(0.0);
        let client = gtk::Label::new(None);
        client.set_xalign(0.0);
        client.add_css_class("dim-label");
        client.add_css_class("caption");
        let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
        column.append(&name);
        column.append(&client);
        item.set_child(Some(&column));
    });
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(project) = projects.get(item.position() as usize) else {
            return;
        };
        let Some(column) = item.child().and_downcast::<gtk::Box>() else {
            return;
        };
        let name = column.first_child().and_downcast::<gtk::Label>();
        let client_label = column.last_child().and_downcast::<gtk::Label>();
        if let (Some(name), Some(client_label)) = (name, client_label) {
            name.set_label(&project.project.name);
            let client = client(project);
            client_label.set_visible(!client.is_empty());
            client_label.set_label(&client);
        }
    });
    factory
}

/// `projects`, with `current` first if it is not among them: an entry on a
/// project no longer assigned can still be looked at, and saved as it is.
pub fn with_current(
    mut projects: Vec<ProjectTargets>,
    current: Option<&TimerTarget>,
) -> Vec<ProjectTargets> {
    if let Some(current) = current {
        match projects
            .iter_mut()
            .find(|project| project.project.id == current.project.id)
        {
            Some(project) => {
                if !project.tasks.iter().any(|task| task.id == current.task.id) {
                    project.tasks.insert(0, current.task.clone());
                }
            }
            None => projects.insert(
                0,
                ProjectTargets {
                    project: current.project.clone(),
                    client: current.client.clone(),
                    tasks: vec![current.task.clone()],
                },
            ),
        }
    }
    projects
}

impl Chooser {
    /// Rows for `projects`, starting on `current` if given, else on the first.
    pub fn new(projects: Vec<ProjectTargets>, current: Option<&TimerTarget>) -> Self {
        let projects = Rc::new(with_current(projects, current));
        let names: Vec<&str> = projects
            .iter()
            .map(|project| project.project.name.as_str())
            .collect();
        let project = adw::ComboRow::builder()
            .title("Project")
            .model(&gtk::StringList::new(&names))
            .list_factory(&project_list(projects.clone()))
            .enable_search(true)
            .build();
        let task = adw::ComboRow::builder().title("Task").build();

        let starting = current
            .and_then(|current| {
                projects
                    .iter()
                    .position(|project| project.project.id == current.project.id)
            })
            .unwrap_or(0);
        let show_tasks = {
            let projects = projects.clone();
            let task = task.clone();
            move |index: u32, choose: Option<i64>| {
                let tasks = projects
                    .get(index as usize)
                    .map(|project| project.tasks.clone())
                    .unwrap_or_default();
                let names: Vec<&str> = tasks.iter().map(|task| task.name.as_str()).collect();
                task.set_model(Some(&gtk::StringList::new(&names)));
                let at = choose
                    .and_then(|id| tasks.iter().position(|task| task.id == id))
                    .unwrap_or(0);
                task.set_selected(at as u32);
            }
        };
        let show_client = {
            let projects = projects.clone();
            move |row: &adw::ComboRow| {
                let client = projects
                    .get(row.selected() as usize)
                    .map(client)
                    .unwrap_or_default();
                row.set_subtitle(&gtk::glib::markup_escape_text(&client));
            }
        };
        project.set_selected(starting as u32);
        show_client(&project);
        show_tasks(starting as u32, current.map(|current| current.task.id));
        project.connect_selected_notify(move |row| {
            show_client(row);
            show_tasks(row.selected(), None);
        });

        Self {
            project,
            task,
            projects,
        }
    }

    /// The project and task chosen, or `None` when there is nothing to choose.
    pub fn selected(&self) -> Option<TimerTarget> {
        let project = self.projects.get(self.project.selected() as usize)?;
        let task = project.tasks.get(self.task.selected() as usize)?;
        Some(project.target(task.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheaves_core::models::Reference;

    fn project(id: i64, tasks: &[i64]) -> ProjectTargets {
        ProjectTargets {
            project: Reference::new(id, format!("Project {id}")),
            client: None,
            tasks: tasks
                .iter()
                .map(|task| Reference::new(*task, format!("Task {task}")))
                .collect(),
        }
    }

    fn target(project: i64, task: i64) -> TimerTarget {
        TimerTarget {
            project: Reference::new(project, format!("Project {project}")),
            task: Reference::new(task, format!("Task {task}")),
            client: None,
        }
    }

    #[test]
    fn an_assigned_project_and_task_are_left_as_they_are() {
        let projects = vec![project(1, &[10, 11]), project(2, &[20])];
        assert_eq!(
            with_current(projects.clone(), Some(&target(2, 20))),
            projects
        );
    }

    /// An entry on a project or task no longer assigned keeps it on offer, so
    /// saving its notes does not move it somewhere else.
    #[test]
    fn a_project_or_task_no_longer_assigned_stays_on_offer() {
        let projects = vec![project(1, &[10])];
        let with_task = with_current(projects.clone(), Some(&target(1, 12)));
        assert_eq!(
            with_task[0].tasks.iter().map(|t| t.id).collect::<Vec<_>>(),
            [12, 10]
        );
        let with_project = with_current(projects, Some(&target(9, 90)));
        assert_eq!(with_project[0].project.id, 9);
        assert_eq!(with_project.len(), 2);
    }
}
