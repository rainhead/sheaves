//! The last known state, so the UI can draw before the first reply arrives.

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::budget::ProjectBudget;
use crate::models::{HarvestCompany, HarvestUser};
use crate::persistence;
use crate::tracked_entry::{TimerTarget, TrackedEntry};

/// Everything the UI needs to draw itself before the first network reply arrives.
///
/// Every field has a default, so that adding a field does not invalidate every cache
/// written before it existed. A required new field would make the first launch after
/// it was added fail to read the cache, and the panel would open empty: losing the
/// entries, the recents and the usage ranking as well as the field that was added. A
/// cache is exactly the thing that should degrade rather than fail.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CachedSnapshot {
    pub user: Option<HarvestUser>,
    pub company: Option<HarvestCompany>,
    pub targets: Vec<TimerTarget>,
    pub entries: Vec<TrackedEntry>,
    /// Most recently used target ids, newest first. Drives the picker's default
    /// order.
    pub recent_target_ids: Vec<String>,
    /// Target ids ranked by the user's own Harvest history, best first.
    pub frequent_target_ids: Vec<String>,
    /// Only the projects with a budget worth drawing, so an account with none caches
    /// an empty list rather than a row per project.
    pub budgets: Vec<ProjectBudget>,
    pub saved_at: Option<Timestamp>,
}

/// Reads and writes the cache as a single JSON file.
///
/// A whole-file rewrite is fine at this size, since a busy year of one person's
/// entries is well under a megabyte, and it keeps the state on disk impossible to
/// half update.
pub struct SnapshotStore {
    path: PathBuf,
}

impl SnapshotStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `None` rather than an error: a missing or unreadable cache just means the UI
    /// starts empty and waits for the network.
    pub fn load(&self) -> Option<CachedSnapshot> {
        persistence::read_json(&self.path)
    }

    pub fn save(&self, snapshot: &CachedSnapshot) -> std::io::Result<()> {
        persistence::write_json(&self.path, snapshot)
    }

    pub fn clear(&self) {
        persistence::remove(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::BudgetBasis;

    /// A cache is the thing that should degrade rather than fail. Adding `budgets`
    /// made the Swift decoder reject every snapshot written before it, which threw
    /// away the entries, the recents and the usage ranking too, on exactly the launch
    /// where the app most wants them.
    #[test]
    fn decodes_pre_budget_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("snapshot.json");
        std::fs::write(
            &path,
            r#"{
              "targets": [],
              "entries": [],
              "recent_target_ids": ["1:10"],
              "frequent_target_ids": [],
              "saved_at": "2026-08-29T12:00:00Z"
            }"#,
        )
        .unwrap();

        let restored = SnapshotStore::new(&path).load().unwrap();
        assert_eq!(restored.recent_target_ids, ["1:10"]);
        assert!(restored.budgets.is_empty());
    }

    #[test]
    fn round_trips_budgets() {
        let directory = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(directory.path().join("snapshot.json"));
        store
            .save(&CachedSnapshot {
                budgets: vec![ProjectBudget::new(
                    7,
                    "P",
                    BudgetBasis::Project,
                    Some(40.0),
                    Some(32.0),
                    Some(8.0),
                )],
                ..CachedSnapshot::default()
            })
            .unwrap();
        assert_eq!(store.load().unwrap().budgets[0].budget_remaining, Some(8.0));
    }
}
