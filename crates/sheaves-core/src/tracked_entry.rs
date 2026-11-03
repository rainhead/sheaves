//! Time entries as Sheaves shows them, and the project and task pairs a timer can
//! run against.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::calendar_date::CalendarDate;
use crate::models::{ProjectAssignment, Reference, TimeEntry, hours_between};

/// An entry's identity: Harvest's id, or one minted here for an entry started while
/// offline, which is swapped for the real one once the create lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryId {
    Server(i64),
    Local(Uuid),
}

impl EntryId {
    pub fn server_id(self) -> Option<i64> {
        match self {
            EntryId::Server(id) => Some(id),
            EntryId::Local(_) => None,
        }
    }
}

/// A time entry as Sheaves shows it, which is not quite what Harvest returns.
///
/// An entry started while offline has no Harvest id yet, so identity is either a
/// server id or a locally minted one. Everything downstream, from the menu bar to
/// the mutation queue, can then treat the two alike.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackedEntry {
    pub id: EntryId,
    pub project: Reference,
    pub task: Reference,
    pub client: Option<Reference>,
    pub notes: Option<String>,
    pub spent_date: CalendarDate,
    /// Hours banked before the current run. A running entry adds live elapsed time
    /// on top.
    pub banked_hours: f64,
    pub is_running: bool,
    pub timer_started_at: Option<Timestamp>,
    pub is_locked: bool,
    pub is_billable: bool,
    /// True while a local change has not yet been accepted by Harvest.
    pub is_pending: bool,
    pub updated_at: Timestamp,
}

impl TrackedEntry {
    pub fn from_harvest(entry: &TimeEntry) -> Self {
        Self {
            id: EntryId::Server(entry.id),
            project: entry.project.clone(),
            task: entry.task.clone(),
            client: entry.client.clone(),
            notes: entry.notes.clone(),
            spent_date: entry.spent_date,
            banked_hours: entry.hours_without_timer.unwrap_or(if entry.is_running {
                0.0
            } else {
                entry.hours
            }),
            is_running: entry.is_running,
            timer_started_at: entry.timer_started_at,
            is_locked: entry.is_locked,
            is_billable: entry.billable,
            is_pending: false,
            updated_at: entry.updated_at,
        }
    }

    /// Hours on the clock at `now`: banked time plus whatever the running timer has
    /// added.
    pub fn hours(&self, now: Timestamp) -> f64 {
        match (self.is_running, self.timer_started_at) {
            (true, Some(started)) => self.banked_hours + hours_between(started, now),
            _ => self.banked_hours,
        }
    }

    pub fn target(&self) -> TimerTarget {
        TimerTarget {
            project: self.project.clone(),
            task: self.task.clone(),
            client: self.client.clone(),
        }
    }
}

/// A project and task pair a timer can be started against: what the picker
/// searches and what "resume" resumes.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TimerTarget {
    pub project: Reference,
    pub task: Reference,
    pub client: Option<Reference>,
}

impl TimerTarget {
    pub fn id(&self) -> String {
        format!("{}:{}", self.project.id, self.task.id)
    }

    /// "Client · Project": the project alone is often ambiguous across clients.
    pub fn project_label(&self) -> String {
        match &self.client {
            Some(client) if !client.name.is_empty() => {
                format!("{} · {}", client.name, self.project.name)
            }
            _ => self.project.name.clone(),
        }
    }
}

/// A project a timer can be started against, with the tasks available on it.
///
/// The picker lists projects rather than project and task pairs. A project with six
/// tasks was six rows carrying the same second line, which buried how few projects
/// there really are; the task is a choice made within the row instead.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProjectTargets {
    pub project: Reference,
    pub client: Option<Reference>,
    /// Best first, by the same ranking that decides the order of the projects.
    pub tasks: Vec<Reference>,
}

impl ProjectTargets {
    pub fn id(&self) -> i64 {
        self.project.id
    }

    /// The pair to start, given whichever task the row is showing.
    pub fn target(&self, task: Reference) -> TimerTarget {
        TimerTarget {
            project: self.project.clone(),
            task,
            client: self.client.clone(),
        }
    }
}

/// Collapses ranked pairs into one entry per project.
///
/// Order is taken from the input on both axes: a project appears where its
/// best-ranked task put it, and its tasks keep the order they were ranked in. So
/// filtering pairs by a query and grouping afterwards leaves the task that matched
/// at the head of its project's list, which is what lets typing a task name still
/// choose that task.
pub fn group_by_project<'a>(
    targets: impl IntoIterator<Item = &'a TimerTarget>,
) -> Vec<ProjectTargets> {
    let mut grouped: Vec<ProjectTargets> = Vec::new();
    for target in targets {
        let index = match grouped
            .iter()
            .position(|group| group.project.id == target.project.id)
        {
            Some(index) => index,
            None => {
                grouped.push(ProjectTargets {
                    project: target.project.clone(),
                    client: target.client.clone(),
                    tasks: Vec::new(),
                });
                grouped.len() - 1
            }
        };
        let tasks = &mut grouped[index].tasks;
        if !tasks.iter().any(|task| task.id == target.task.id) {
            tasks.push(target.task.clone());
        }
    }
    grouped
}

/// Every active project and task pair the user may log against.
pub fn timer_targets(assignments: &[ProjectAssignment]) -> Vec<TimerTarget> {
    assignments
        .iter()
        .filter(|assignment| assignment.is_active)
        .flat_map(|assignment| {
            let project = Reference::new(assignment.project.id, assignment.project.name.clone());
            assignment
                .task_assignments
                .iter()
                .filter(|task| task.is_active)
                .map(move |task| TimerTarget {
                    project: project.clone(),
                    task: task.task.clone(),
                    client: Some(assignment.client.clone()),
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::offset_by;
    use crate::test_support::fixture;

    fn entry(banked: f64, running: bool, started_at: Option<Timestamp>) -> TrackedEntry {
        TrackedEntry {
            id: EntryId::Server(1),
            project: Reference::new(1, "Project"),
            task: Reference::new(2, "Task"),
            client: None,
            notes: None,
            spent_date: CalendarDate::new(2026, 8, 29),
            banked_hours: banked,
            is_running: running,
            timer_started_at: started_at,
            is_locked: false,
            is_billable: true,
            is_pending: false,
            updated_at: Timestamp::now(),
        }
    }

    mod tracked_entry {
        use super::*;

        #[test]
        fn counts_running_time() {
            let started = Timestamp::now();
            let tracked = entry(1.0, true, Some(started));
            assert_eq!(tracked.hours(offset_by(started, 1800.0)), 1.5);
        }

        #[test]
        fn stopped_entry() {
            let tracked = entry(2.25, false, None);
            assert_eq!(tracked.hours(offset_by(Timestamp::now(), 3600.0)), 2.25);
        }

        /// Clock changes can put `now` behind `timer_started_at`; a negative
        /// duration in the menu bar would be worse than a stalled one.
        #[test]
        fn clamps_backwards_clock() {
            let started = Timestamp::now();
            let tracked = entry(1.0, true, Some(started));
            assert_eq!(tracked.hours(offset_by(started, -600.0)), 1.0);
        }

        /// Harvest's `hours` on a running entry is a snapshot; `hours_without_timer`
        /// is the part that is safe to add live elapsed time to.
        #[test]
        fn converts_running_entry() {
            let entry: TimeEntry = serde_json::from_str(&fixture::running_time_entry()).unwrap();
            let tracked = TrackedEntry::from_harvest(&entry);
            assert!(tracked.is_running);
            assert_eq!(tracked.banked_hours, 1.0);
            assert_eq!(tracked.id, EntryId::Server(636708906));
        }

        #[test]
        fn converts_stopped_entry() {
            let entry: TimeEntry = serde_json::from_str(&fixture::time_entry()).unwrap();
            let tracked = TrackedEntry::from_harvest(&entry);
            assert!(!tracked.is_running);
            assert_eq!(tracked.banked_hours, 2.11);
            assert_eq!(tracked.hours(Timestamp::now()), 2.11);
        }
    }

    mod project_grouping {
        use super::*;

        fn target(project: i64, project_name: &str, task: i64, task_name: &str) -> TimerTarget {
            TimerTarget {
                project: Reference::new(project, project_name),
                task: Reference::new(task, task_name),
                client: Some(Reference::new(900 + project, format!("Client {project}"))),
            }
        }

        fn names(references: &[Reference]) -> Vec<&str> {
            references
                .iter()
                .map(|reference| reference.name.as_str())
                .collect()
        }

        /// The picker shows one row per project, so the pairs have to collapse
        /// without losing the ranking that put them in this order.
        #[test]
        fn groups_in_ranked_order() {
            let targets = [
                target(2, "Second", 20, "Programming"),
                target(1, "First", 10, "Design"),
                target(2, "Second", 21, "QA"),
                target(1, "First", 11, "Research"),
            ];
            let grouped = group_by_project(&targets);
            let projects: Vec<&str> = grouped
                .iter()
                .map(|group| group.project.name.as_str())
                .collect();
            assert_eq!(projects, ["Second", "First"]);
            assert_eq!(names(&grouped[0].tasks), ["Programming", "QA"]);
            assert_eq!(names(&grouped[1].tasks), ["Design", "Research"]);
            assert_eq!(grouped[0].client.as_ref().unwrap().name, "Client 2");
        }

        #[test]
        fn dedupes_tasks() {
            let targets = [
                target(1, "First", 10, "Design"),
                target(1, "First", 10, "Design"),
                target(1, "First", 11, "Research"),
            ];
            let grouped = group_by_project(&targets);
            assert_eq!(grouped.len(), 1);
            assert_eq!(names(&grouped[0].tasks), ["Design", "Research"]);
        }

        #[test]
        fn identified_by_project() {
            let grouped = group_by_project(&[target(1, "First", 10, "Design")]);
            assert_eq!(grouped[0].id(), 1);
            assert_eq!(grouped[0].target(Reference::new(10, "Design")).id(), "1:10");
        }
    }
}
