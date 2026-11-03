//! Local changes waiting to reach Harvest, persisted and sent in order.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::calendar_date::CalendarDate;
use crate::client::{EntryUpdate, HarvestClient, HarvestError};
use crate::clock::Clock;
use crate::models::{TimeEntry, hours_between};
use crate::persistence;
use crate::tracked_entry::{EntryId, TimerTarget};

/// A change the user made locally that Harvest has not accepted yet.
///
/// Every case that affects elapsed time carries the time itself, not just the
/// command. Harvest's `/stop` banks time up to the moment the request *arrives*, and
/// a create starts its timer when it arrives too, so replaying bare commands after a
/// spell offline silently destroys or invents hours. Recording when the user
/// actually acted, and sending explicit hours, keeps the entry honest however late
/// the request lands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Mutation {
    /// `ended_at == None` means the timer was still running when this was queued;
    /// the entry is created with the elapsed time so far and then resumed.
    Create {
        local: Uuid,
        target: TimerTarget,
        spent_date: CalendarDate,
        notes: Option<String>,
        started_at: Timestamp,
        ended_at: Option<Timestamp>,
    },
    /// `hours` is the total measured locally when the user stopped the timer.
    Stop {
        id: EntryId,
        hours: f64,
    },
    /// `banked_hours` is the total before resuming; `resumed_at` is when the user
    /// did.
    Restart {
        id: EntryId,
        resumed_at: Timestamp,
        banked_hours: f64,
    },
    Update {
        id: EntryId,
        notes: Option<String>,
        hours: Option<f64>,
    },
    /// A new total typed onto a timer still running. Harvest sets the total and
    /// counts on from the moment the patch *arrives*, so `as_of`, when the user
    /// chose the number, is folded in as elapsed time however late that is.
    Adjust {
        id: EntryId,
        hours: f64,
        as_of: Timestamp,
    },
    Delete {
        id: EntryId,
    },
}

impl Mutation {
    /// The entry this mutation acts on, so the queue can rewrite local ids once the
    /// create that minted them has landed.
    pub fn subject(&self) -> EntryId {
        match self {
            Mutation::Create { local, .. } => EntryId::Local(*local),
            Mutation::Stop { id, .. }
            | Mutation::Restart { id, .. }
            | Mutation::Update { id, .. }
            | Mutation::Adjust { id, .. }
            | Mutation::Delete { id } => *id,
        }
    }

    fn retargeted(&self, to: EntryId) -> Mutation {
        let mut copy = self.clone();
        match &mut copy {
            Mutation::Create { .. } => {}
            Mutation::Stop { id, .. }
            | Mutation::Restart { id, .. }
            | Mutation::Update { id, .. }
            | Mutation::Adjust { id, .. }
            | Mutation::Delete { id } => *id = to,
        }
        copy
    }
}

#[derive(Debug, Default)]
pub struct DrainReport {
    /// Mutations Harvest accepted.
    pub applied: usize,
    /// Local ids that earned a Harvest id during this drain, so the tracker can
    /// adopt them without waiting for a refetch.
    pub resolved: HashMap<Uuid, i64>,
    /// Mutations Harvest refused outright; these were dropped from the queue.
    pub discarded: Vec<(Mutation, HarvestError)>,
    /// Set when the queue stopped early because the failure looked temporary.
    pub stopped_with: Option<HarvestError>,
}

impl DrainReport {
    pub fn is_empty(&self) -> bool {
        self.applied == 0 && self.discarded.is_empty() && self.stopped_with.is_none()
    }
}

/// The Harvest account a queue's mutations were made under, and by whom.
///
/// The user is part of it because one account holds many people, and an
/// administrator's token *can* stop or delete a colleague's entry: across users a
/// stale mutation does not fail, it succeeds against the wrong person.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Owner {
    pub account_id: String,
    pub user_id: i64,
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    pending: Vec<Mutation>,
    /// Local ids that have earned a Harvest id, newest last.
    ///
    /// Rewriting only the mutations already queued is not enough: the user can stop
    /// a timer after its create drained but before the tracker has adopted the real
    /// id, and that stop would target a local id nothing recognises. It would be
    /// discarded as "not found" and the timer would run on Harvest indefinitely.
    #[serde(default)]
    resolved: Vec<Resolution>,
    /// Whose work this is, or `None` for a queue nobody has claimed yet.
    ///
    /// Queued mutations name entries, projects and tasks by ids that mean something
    /// only to the account that minted them, and only the user who made them may be
    /// assumed to want them sent. Credentials can change without `remove_all` ever
    /// running, an expired token and then a different one pasted into Settings, so
    /// the queue has to remember this for itself.
    #[serde(default)]
    owner: Option<Owner>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct Resolution {
    local: Uuid,
    server_id: i64,
}

/// An ordered, persisted list of local changes waiting to reach Harvest.
///
/// Order matters, since stopping a timer that a queued create has not yet made real
/// would otherwise fail, so the queue drains strictly in sequence and stops at the
/// first failure that a retry could fix. Failures a retry could *not* fix, such as a
/// locked entry or a deleted project, drop that one mutation and let the rest
/// through, because otherwise a single bad change would wedge every later one
/// forever.
///
/// The state is locked only between requests, never across one, so the queue can be
/// changed while a drain waits on the network: a mutation enqueued meanwhile is
/// sent in the same drain, and `remove_all` on disconnect empties it for good.
pub struct MutationQueue {
    state: Mutex<Stored>,
    is_draining: AtomicBool,
    path: PathBuf,
    clock: Arc<dyn Clock>,
}

/// Enough history to cover any plausible in-flight mutation without growing forever.
const RESOLVED_LIMIT: usize = 200;

impl MutationQueue {
    /// Opens the queue persisted at `path`, or an empty one when there is none or it
    /// cannot be read.
    pub fn open(path: impl Into<PathBuf>, clock: Arc<dyn Clock>) -> Self {
        let path = path.into();
        let state = persistence::read_json_or_set_aside(&path).unwrap_or_default();
        Self {
            state: Mutex::new(state),
            is_draining: AtomicBool::new(false),
            path,
            clock,
        }
    }

    pub fn count(&self) -> usize {
        self.state.lock().unwrap().pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    pub fn owner(&self) -> Option<Owner> {
        self.state.lock().unwrap().owner.clone()
    }

    pub fn pending(&self) -> Vec<Mutation> {
        self.state.lock().unwrap().pending.clone()
    }

    pub fn enqueue(&self, mutation: Mutation) {
        self.change(|state| state.pending.push(mutation));
    }

    pub fn remove_all(&self) {
        self.change(|state| *state = Stored::default());
    }

    /// How many queued mutations `owner` must not send: all of them when the queue
    /// is someone else's, none when it is theirs or nobody's.
    pub fn stranded_count(&self, owner: &Owner) -> usize {
        let state = self.state.lock().unwrap();
        match &state.owner {
            Some(current) if current != owner => state.pending.len(),
            _ => 0,
        }
    }

    /// Makes the queue `owner`'s. Their own work, and work nobody has claimed, is
    /// kept; anyone else's is discarded, so ask `stranded_count` first and let the
    /// user decide.
    pub fn claim(&self, owner: Owner) {
        self.change(|state| {
            if state
                .owner
                .as_ref()
                .is_some_and(|current| *current != owner)
            {
                state.pending.clear();
                state.resolved.clear();
            }
            state.owner = Some(owner);
        });
    }

    /// Names the owner of a queue that has none, and leaves any other alone.
    pub fn claim_if_unowned(&self, owner: Owner) {
        if self.owner().is_none() {
            self.claim(owner);
        }
    }

    /// Marks a queued create as finished, or running again, without adding a second
    /// mutation, for a timer started and stopped before the queue ever drained.
    /// Returns false when no such create is pending, and the caller queues a stop or
    /// a restart instead.
    ///
    /// The create at the head of a queue that is draining may already be on its way
    /// to Harvest, and is refused. Amended in flight, it would no longer match the
    /// copy the drain sent, so the drain would leave it queued and send it again: a
    /// second entry for one timer.
    pub fn amend_create(&self, local: Uuid, ended_at: Option<Timestamp>) -> bool {
        let mut amended = false;
        self.change(|state| {
            for (index, mutation) in state.pending.iter_mut().enumerate() {
                if let Mutation::Create {
                    local: id,
                    ended_at: end,
                    ..
                } = mutation
                    && *id == local
                {
                    // Read under the lock the drain takes to copy the head, so the
                    // two cannot interleave.
                    if index == 0 && self.is_draining.load(Ordering::SeqCst) {
                        return;
                    }
                    *end = ended_at;
                    amended = true;
                    return;
                }
            }
        });
        amended
    }

    /// Sends queued mutations to Harvest in order, stopping at the first retryable
    /// failure.
    pub async fn drain(&self, client: &HarvestClient) -> DrainReport {
        // The loop awaits the network, so a second caller can arrive partway
        // through. Two drains would read the same first mutation, send it twice, and
        // then each remove one, duplicating a mutation and losing its successor.
        if self
            .is_draining
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return DrainReport::default();
        }
        let _guard = DrainGuard(&self.is_draining);
        let mut report = DrainReport::default();

        loop {
            let Some(mutation) = self.state.lock().unwrap().pending.first().cloned() else {
                break;
            };
            let outcome = match &mutation {
                Mutation::Create { local, .. } => {
                    // A create can need a second request, a restart or a stop.
                    // Sending both inside one mutation meant a transient failure of
                    // the second repeated the first: one duplicate entry per retry,
                    // with the original left running. So the create is committed to
                    // the queue the moment the POST succeeds, and the second request
                    // takes its place as an ordinary mutation with ordinary retries.
                    let local = *local;
                    match self.create(&mutation, client).await {
                        Ok((entry, follow_up)) => {
                            self.change(|state| {
                                remember(&mut state.resolved, local, entry.id);
                                // Later queued changes still point at the
                                // placeholder id; repoint them.
                                for queued in &mut state.pending {
                                    if queued.subject() == EntryId::Local(local) {
                                        *queued = queued.retargeted(EntryId::Server(entry.id));
                                    }
                                }
                                // The queue can have been emptied while the request
                                // was in flight, by `remove_all` on disconnect.
                                // Touch it only where the drained mutation stands.
                                if state.pending.first() == Some(&mutation) {
                                    match follow_up {
                                        Some(follow_up) => state.pending[0] = follow_up,
                                        None => {
                                            state.pending.remove(0);
                                        }
                                    }
                                }
                            });
                            report.resolved.insert(local, entry.id);
                            Ok(())
                        }
                        Err(error) => Err(error),
                    }
                }
                _ => match self.apply(&mutation, client).await {
                    Ok(()) => {
                        self.change(|state| {
                            if state.pending.first() == Some(&mutation) {
                                state.pending.remove(0);
                            }
                        });
                        Ok(())
                    }
                    Err(error) => Err(error),
                },
            };
            match outcome {
                Ok(()) => report.applied += 1,
                // A rejected token fails every request alike. Treated as a refusal
                // of this one change, it would discard the whole queue in one drain,
                // and the work a renewed token exists to send would be gone.
                Err(error)
                    if error.is_transient()
                        || matches!(
                            error,
                            HarvestError::Unauthorized | HarvestError::NotConfigured
                        ) =>
                {
                    report.stopped_with = Some(error);
                    break;
                }
                Err(error) => {
                    self.change(|state| {
                        if state.pending.first() == Some(&mutation) {
                            state.pending.remove(0);
                        }
                    });
                    report.discarded.push((mutation, error));
                }
            }
        }
        report
    }

    /// Harvest keeps hours to two decimals, and its parser refuses the scientific
    /// notation that a tiny number encodes to: a sub-second elapsed time comes back
    /// as "would create a negative total for the selected project and task". Every
    /// hours figure is therefore rounded to what Harvest can store before it is sent.
    pub fn storable_hours(hours: f64) -> f64 {
        (hours * 100.0).round() / 100.0
    }

    /// Sends the POST for a queued create and names the mutation that must follow
    /// it, if any. The caller queues that follow-up rather than this method sending
    /// it, so a failure between the two requests cannot repeat the first.
    async fn create(
        &self,
        mutation: &Mutation,
        client: &HarvestClient,
    ) -> Result<(TimeEntry, Option<Mutation>), HarvestError> {
        let Mutation::Create {
            target,
            spent_date,
            notes,
            started_at,
            ended_at,
            ..
        } = mutation
        else {
            unreachable!("create requires a create mutation");
        };
        let now = self.clock.now();
        // Measure from when the user started, not from now.
        let finish = ended_at.unwrap_or(now);
        let hours = Self::storable_hours(hours_between(*started_at, finish));
        let (project, task) = (target.project.id, target.task.id);
        if hours <= 0.0 {
            // Nothing measurable to bank, and an explicit 0 would itself start the
            // timer. Create the entry running, which is what a fresh click wants,
            // and stop it again when the user already had.
            let entry = client
                .create_time_entry(project, task, *spent_date, notes.as_deref(), None)
                .await?;
            let follow_up = ended_at.map(|_| Mutation::Stop {
                id: EntryId::Server(entry.id),
                hours: 0.0,
            });
            return Ok((entry, follow_up));
        }
        let entry = client
            .create_time_entry(project, task, *spent_date, notes.as_deref(), Some(hours))
            .await?;
        if ended_at.is_some() {
            return Ok((entry, None));
        }
        // Still running: the entry holds the offline time; the restart lets Harvest
        // count onwards, banking whatever more accrues before it lands.
        let follow_up = Mutation::Restart {
            id: EntryId::Server(entry.id),
            resumed_at: now,
            banked_hours: hours,
        };
        Ok((entry, Some(follow_up)))
    }

    async fn apply(&self, mutation: &Mutation, client: &HarvestClient) -> Result<(), HarvestError> {
        let server_id = self
            .server_id(mutation.subject())
            .ok_or(HarvestError::NotFound)?;
        let now = self.clock.now();
        match mutation {
            Mutation::Create { .. } => unreachable!("creates go through create()"),
            Mutation::Stop { hours, .. } => {
                match client.stop_time_entry(server_id).await {
                    // Already stopped on the server: the hours still need correcting.
                    Ok(_) | Err(HarvestError::Rejected { .. }) => {}
                    Err(error) => return Err(error),
                }
                client
                    .update_time_entry(server_id, hours_only(Self::storable_hours(*hours)))
                    .await?;
            }
            Mutation::Restart {
                resumed_at,
                banked_hours,
                ..
            } => {
                let offline = hours_between(*resumed_at, now);
                if offline > 1.0 / 3600.0 {
                    // Bank the time it ran offline *before* resuming, so the total is
                    // never patched while the timer is live.
                    client
                        .update_time_entry(
                            server_id,
                            hours_only(Self::storable_hours(banked_hours + offline)),
                        )
                        .await?;
                }
                client.restart_time_entry(server_id).await?;
            }
            Mutation::Update { notes, hours, .. } => {
                client
                    .update_time_entry(
                        server_id,
                        EntryUpdate {
                            notes: notes.as_deref(),
                            hours: hours.map(Self::storable_hours),
                            ..EntryUpdate::default()
                        },
                    )
                    .await?;
            }
            Mutation::Adjust { hours, as_of, .. } => {
                let elapsed = hours_between(*as_of, now);
                client
                    .update_time_entry(server_id, hours_only(Self::storable_hours(hours + elapsed)))
                    .await?;
            }
            Mutation::Delete { .. } => client.delete_time_entry(server_id).await?,
        }
        Ok(())
    }

    /// The Harvest id a local id earned, if this queue has seen it created.
    fn server_id(&self, id: EntryId) -> Option<i64> {
        match id {
            EntryId::Server(id) => Some(id),
            EntryId::Local(local) => self
                .state
                .lock()
                .unwrap()
                .resolved
                .iter()
                .rev()
                .find(|resolution| resolution.local == local)
                .map(|resolution| resolution.server_id),
        }
    }

    /// Applies `change` to the state and persists the result.
    fn change(&self, change: impl FnOnce(&mut Stored)) {
        let mut state = self.state.lock().unwrap();
        change(&mut state);
        if let Err(error) = persistence::write_json(&self.path, &*state) {
            log::error!("could not save the queue: {error}");
        }
    }
}

fn hours_only(hours: f64) -> EntryUpdate<'static> {
    EntryUpdate {
        hours: Some(hours),
        ..EntryUpdate::default()
    }
}

fn remember(resolved: &mut Vec<Resolution>, local: Uuid, server_id: i64) {
    resolved.retain(|resolution| resolution.local != local);
    resolved.push(Resolution { local, server_id });
    if resolved.len() > RESOLVED_LIMIT {
        let excess = resolved.len() - RESOLVED_LIMIT;
        resolved.drain(..excess);
    }
}

/// Clears the draining flag however a drain ends, including by being dropped.
struct DrainGuard<'a>(&'a AtomicBool);

impl Drop for DrainGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Method;
    use crate::clock::SystemClock;
    use crate::models::{Reference, offset_by};
    use crate::test_support::fixture::{self, today};
    use crate::test_support::{GatedTransport, Route, RoutingTransport, Scripted, StubTransport};

    struct Scratch {
        _directory: tempfile::TempDir,
        path: PathBuf,
    }

    fn scratch() -> Scratch {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("queue.json");
        Scratch {
            _directory: directory,
            path,
        }
    }

    fn open(scratch: &Scratch) -> MutationQueue {
        MutationQueue::open(&scratch.path, Arc::new(SystemClock))
    }

    fn client(transport: Arc<dyn crate::client::Transport>) -> HarvestClient {
        HarvestClient::new(Some(fixture::credentials()), transport, 0.0)
    }

    fn target() -> TimerTarget {
        TimerTarget {
            project: Reference::new(14308069, "Online Store - Phase 1"),
            task: Reference::new(8083366, "Programming"),
            client: Some(Reference::new(5735776, "123 Industries")),
        }
    }

    fn create(local: Uuid, started_at: Timestamp, ended_at: Option<Timestamp>) -> Mutation {
        Mutation::Create {
            local,
            target: target(),
            spent_date: today(),
            notes: None,
            started_at,
            ended_at,
        }
    }

    fn ago(seconds: f64) -> Timestamp {
        offset_by(Timestamp::now(), -seconds)
    }

    fn writable_account() -> Arc<RoutingTransport> {
        RoutingTransport::new(vec![
            Route::new(Method::Patch, "/stop", &fixture::time_entry()),
            Route::new(Method::Patch, "/restart", &fixture::running_time_entry()),
            Route::new(Method::Post, "time_entries", &fixture::time_entry()).status(201),
            Route::new(Method::Patch, "time_entries", &fixture::time_entry()),
        ])
    }

    /// The update a stop sends after `/stop`, which corrects the hours.
    fn correction(transport: &RoutingTransport) -> crate::test_support::Call {
        transport
            .calls_to(Method::Patch, "time_entries")
            .into_iter()
            .find(|call| !call.path.contains("/stop") && !call.path.contains("/restart"))
            .expect("an hours correction")
    }

    mod mutation_queue {
        use super::*;

        #[tokio::test]
        async fn drains_in_order() {
            let scratch = scratch();
            let queue = open(&scratch);
            let transport = StubTransport::new(vec![Scripted::ok(&fixture::time_entry()); 3]);

            queue.enqueue(Mutation::Update {
                id: EntryId::Server(636709355),
                notes: Some("first".to_string()),
                hours: None,
            });
            queue.enqueue(Mutation::Stop {
                id: EntryId::Server(636709355),
                hours: 1.0,
            });
            let report = queue.drain(&client(transport.clone())).await;

            assert_eq!(report.applied, 2);
            assert!(queue.is_empty());
            assert_eq!(
                transport.request(0).url.path(),
                "/v2/time_entries/636709355"
            );
            assert_eq!(
                transport.request(1).url.path(),
                "/v2/time_entries/636709355/stop"
            );
        }

        /// Starting a timer offline and stopping it before reconnecting: the stop
        /// refers to an entry Harvest has never seen, so the queue has to repoint it
        /// once the create returns a real id.
        #[tokio::test]
        async fn rewrites_local_identifiers() {
            let scratch = scratch();
            let queue = open(&scratch);
            let transport = StubTransport::new(vec![
                Scripted::status(201, &fixture::running_time_entry()), // create
                Scripted::ok(&fixture::time_entry()),                  // stop
                Scripted::ok(&fixture::time_entry()),                  // hours correction
            ]);

            let local = Uuid::new_v4();
            queue.enqueue(create(local, ago(1800.0), Some(Timestamp::now())));
            queue.enqueue(Mutation::Stop {
                id: EntryId::Local(local),
                hours: 0.5,
            });
            let report = queue.drain(&client(transport.clone())).await;

            assert_eq!(report.applied, 2);
            // 636708906 is the id the stubbed create returned.
            assert_eq!(
                transport.request(1).url.path(),
                "/v2/time_entries/636708906/stop"
            );
        }

        /// A dropped connection must not lose the change; it stays queued for next
        /// time.
        #[tokio::test]
        async fn halts_on_transient_failure() {
            let scratch = scratch();
            let queue = open(&scratch);
            let transport = StubTransport::new(vec![Scripted::status(500, ""); 4]);

            queue.enqueue(Mutation::Stop {
                id: EntryId::Server(1),
                hours: 1.0,
            });
            queue.enqueue(Mutation::Stop {
                id: EntryId::Server(2),
                hours: 1.0,
            });
            let report = queue.drain(&client(transport)).await;

            assert_eq!(report.applied, 0);
            assert!(
                report
                    .stopped_with
                    .is_some_and(|error| error.is_transient())
            );
            assert_eq!(queue.count(), 2);
        }

        /// A change Harvest will never accept, a locked entry or a deleted project,
        /// would otherwise wedge every later change behind it forever.
        #[tokio::test]
        async fn discards_permanent_failure() {
            let scratch = scratch();
            let queue = open(&scratch);
            let transport = StubTransport::new(vec![
                Scripted::status(422, r#"{"message": "Time entry is locked."}"#),
                Scripted::ok(&fixture::time_entry()),
                Scripted::ok(&fixture::time_entry()),
            ]);

            queue.enqueue(Mutation::Update {
                id: EntryId::Server(1),
                notes: Some("nope".to_string()),
                hours: None,
            });
            queue.enqueue(Mutation::Stop {
                id: EntryId::Server(2),
                hours: 1.0,
            });
            let report = queue.drain(&client(transport)).await;

            assert_eq!(report.applied, 1);
            assert_eq!(report.discarded.len(), 1);
            assert!(queue.is_empty());
        }

        /// A rejected token fails every request alike, so it must stop the drain
        /// with everything kept for the renewed token, not discard the queue one
        /// change at a time.
        #[tokio::test]
        async fn keeps_everything_when_the_token_is_rejected() {
            let scratch = scratch();
            let queue = open(&scratch);
            let transport = StubTransport::new(vec![Scripted::status(401, ""); 2]);

            queue.enqueue(Mutation::Stop {
                id: EntryId::Server(1),
                hours: 1.0,
            });
            queue.enqueue(Mutation::Stop {
                id: EntryId::Server(2),
                hours: 1.0,
            });
            let report = queue.drain(&client(transport.clone())).await;

            assert_eq!(report.stopped_with, Some(HarvestError::Unauthorized));
            assert!(report.discarded.is_empty());
            assert_eq!(queue.count(), 2);
            assert_eq!(transport.request_count(), 1);
        }

        /// A refusal of one change, by contrast, is that change's alone.
        #[tokio::test]
        async fn drops_only_a_forbidden_change() {
            let scratch = scratch();
            let queue = open(&scratch);
            let transport = StubTransport::new(vec![
                Scripted::status(403, ""),
                Scripted::ok(&fixture::time_entry()),
                Scripted::ok(&fixture::time_entry()),
            ]);

            queue.enqueue(Mutation::Delete {
                id: EntryId::Server(1),
            });
            queue.enqueue(Mutation::Stop {
                id: EntryId::Server(2),
                hours: 1.0,
            });
            let report = queue.drain(&client(transport)).await;

            assert_eq!(report.discarded.len(), 1);
            assert_eq!(report.applied, 1);
            assert!(queue.is_empty());
        }

        /// A queue file this build cannot read, such as one a newer build wrote, is
        /// kept beside the new one rather than overwritten by it.
        #[test]
        fn sets_aside_an_unreadable_file() {
            let scratch = scratch();
            std::fs::write(&scratch.path, r#"{"pending": [{"kind": "teleport"}]}"#).unwrap();

            let queue = open(&scratch);
            assert!(queue.is_empty());
            queue.enqueue(Mutation::Delete {
                id: EntryId::Server(1),
            });

            let kept: Vec<_> = std::fs::read_dir(scratch.path.parent().unwrap())
                .unwrap()
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.file_name().to_string_lossy().contains(".unreadable-"))
                .collect();
            assert_eq!(kept.len(), 1);
            assert!(
                std::fs::read_to_string(kept[0].path())
                    .unwrap()
                    .contains("teleport")
            );
        }

        #[test]
        fn persists_across_launches() {
            let scratch = scratch();
            let first = open(&scratch);
            first.enqueue(Mutation::Stop {
                id: EntryId::Server(636709355),
                hours: 1.0,
            });
            assert_eq!(first.count(), 1);

            let reopened = open(&scratch);
            assert_eq!(reopened.count(), 1);

            reopened.remove_all();
            assert!(open(&scratch).is_empty());
        }
    }

    mod draining_under_mutation {
        use super::*;

        /// Stopping a timer while its create is on the wire. Amending the create
        /// then would make it differ from the copy the drain sent, so the drain would
        /// leave it queued and post it a second time. Refused, the caller queues a
        /// stop instead, which the remembered resolution delivers.
        #[tokio::test]
        async fn amend_refused_while_the_create_is_in_flight() {
            let scratch = scratch();
            let queue = Arc::new(open(&scratch));
            let local = Uuid::new_v4();
            queue.enqueue(create(local, ago(3600.0), None));

            let transport = GatedTransport::new(&fixture::running_time_entry());
            let client = Arc::new(client(transport.clone()));
            let drain = {
                let (queue, client) = (queue.clone(), client.clone());
                tokio::spawn(async move { queue.drain(&client).await })
            };
            while transport.arrived() == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }

            assert!(!queue.amend_create(local, Some(Timestamp::now())));
            transport.release_all();
            drain.await.unwrap();

            // One POST, then the restart that followed it, and no second create.
            assert_eq!(transport.arrived(), 2);
            assert!(queue.is_empty());
        }

        /// The queue is free to be changed while a request is in flight:
        /// disconnecting mid-drain calls `remove_all` under a waiting drain. The
        /// landing request must notice the queue changed under it, not trap on it.
        #[tokio::test]
        async fn remove_all_during_flight() {
            let scratch = scratch();
            let queue = Arc::new(open(&scratch));
            queue.enqueue(create(Uuid::new_v4(), ago(3600.0), None));

            let transport = GatedTransport::new(&fixture::time_entry());
            let client = Arc::new(client(transport.clone()));
            let drain = {
                let (queue, client) = (queue.clone(), client.clone());
                tokio::spawn(async move { queue.drain(&client).await })
            };
            while transport.arrived() == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }

            // Disconnect while the create is in flight, then let it land.
            queue.remove_all();
            transport.release_all();
            let report = drain.await.unwrap();

            // The POST reached Harvest, but the cleared queue stays cleared: no
            // resurrected follow-up, and above all no trap on the empty list.
            assert_eq!(report.applied, 1);
            assert!(queue.is_empty());
        }
    }

    /// Harvest's `/stop` banks time up to the moment the request *arrives*, and a
    /// create starts its timer on arrival too. Replaying bare commands after a spell
    /// offline therefore destroys or invents hours. These tests pin down the fix:
    /// the mutation carries the time the user actually acted, and the queue sends
    /// explicit hours.
    mod offline_timing {
        use super::*;

        /// Started 2:00, stopped 2:30, delivered at 4:00. Before the fix this created
        /// the entry at 4:00 and stopped it immediately: half an hour of work became
        /// zero.
        #[tokio::test]
        async fn create_carries_measured_length() {
            let scratch = scratch();
            let queue = open(&scratch);
            let transport = writable_account();

            let started_at = ago(2.0 * 3600.0);
            queue.enqueue(create(
                Uuid::new_v4(),
                started_at,
                Some(offset_by(started_at, 1800.0)),
            ));
            let report = queue.drain(&client(transport.clone())).await;

            assert_eq!(report.applied, 1);
            let post = &transport.calls_to(Method::Post, "time_entries")[0];
            assert_eq!(post.hours, Some(0.5));
            // A finished entry must not be resumed.
            assert!(transport.calls_to(Method::Patch, "/restart").is_empty());
        }

        /// A timer still running when the queue drains keeps the time it ran
        /// offline and then carries on counting.
        #[tokio::test]
        async fn still_running_create_banks_then_resumes() {
            let scratch = scratch();
            let queue = open(&scratch);
            let transport = writable_account();

            queue.enqueue(create(Uuid::new_v4(), ago(3600.0), None));
            queue.drain(&client(transport.clone())).await;

            let hours = transport.calls_to(Method::Post, "time_entries")[0]
                .hours
                .unwrap();
            assert!((hours - 1.0).abs() < 0.01);
            assert_eq!(transport.calls_to(Method::Patch, "/restart").len(), 1);
        }

        /// Running since 9:00, network drops, user stops at 9:30, reconnects at
        /// 11:00. Harvest's own stop would bank two hours; the recorded half hour
        /// must win.
        #[tokio::test]
        async fn stop_corrects_hours() {
            let scratch = scratch();
            let queue = open(&scratch);
            let transport = writable_account();

            queue.enqueue(Mutation::Stop {
                id: EntryId::Server(636709355),
                hours: 0.5,
            });
            queue.drain(&client(transport.clone())).await;

            assert_eq!(transport.calls_to(Method::Patch, "/stop").len(), 1);
            assert_eq!(correction(&transport).hours, Some(0.5));
        }

        /// Resuming offline must add the time it ran before the request landed, and
        /// the total must be set while the entry is still stopped.
        #[tokio::test]
        async fn restart_banks_offline_time_first() {
            let scratch = scratch();
            let queue = open(&scratch);
            let transport = writable_account();

            queue.enqueue(Mutation::Restart {
                id: EntryId::Server(636709355),
                resumed_at: ago(1800.0),
                banked_hours: 1.0,
            });
            queue.drain(&client(transport.clone())).await;

            let patch = transport
                .first_index_excluding(Method::Patch, "/restart")
                .unwrap();
            let restart = transport.first_index(Method::Patch, "/restart").unwrap();
            assert!(
                patch < restart,
                "hours must be set before the timer resumes"
            );
            assert!((transport.hours_at(patch).unwrap() - 1.5).abs() < 0.01);
        }

        /// A click that reaches Harvest within seconds has nothing measurable to
        /// bank. Sending the sub-second elapsed time used to encode as scientific
        /// notation (`2.8e-05`), which Harvest refuses as "a negative total". The
        /// entry is created running instead, with no hours at all.
        #[tokio::test]
        async fn fresh_click_sends_no_hours() {
            let scratch = scratch();
            let queue = open(&scratch);
            let transport = writable_account();

            queue.enqueue(create(Uuid::new_v4(), Timestamp::now(), None));
            let report = queue.drain(&client(transport.clone())).await;

            assert_eq!(report.applied, 1);
            assert!(report.discarded.is_empty());
            assert_eq!(
                transport.calls_to(Method::Post, "time_entries")[0].hours,
                None
            );
            // Born running; restarting it would be refused as already running.
            assert!(transport.calls_to(Method::Patch, "/restart").is_empty());
        }

        /// The same trap for a timer already finished: an explicit 0 starts the
        /// timer running on Harvest, so a sub-minute finished entry is created
        /// running and stopped again rather than sent as a sliver of hours.
        #[tokio::test]
        async fn tiny_finished_entry_is_created_then_stopped() {
            let scratch = scratch();
            let queue = open(&scratch);
            let transport = writable_account();

            let started_at = ago(5.0);
            queue.enqueue(create(
                Uuid::new_v4(),
                started_at,
                Some(offset_by(started_at, 5.0)),
            ));
            let report = queue.drain(&client(transport.clone())).await;

            // Two mutations by the time it drains: the create, then the stop that
            // replaced it in the queue once the POST had landed.
            assert_eq!(report.applied, 2);
            assert!(report.discarded.is_empty());
            assert_eq!(
                transport.calls_to(Method::Post, "time_entries")[0].hours,
                None
            );
            assert_eq!(transport.calls_to(Method::Patch, "/stop").len(), 1);
            assert!(transport.calls_to(Method::Patch, "/restart").is_empty());
        }

        /// Harvest stores two decimals; anything finer encodes unpredictably and
        /// buys nothing. 456 seconds is 0.126666… hours and must go out as 0.13.
        #[tokio::test]
        async fn hours_are_rounded_to_harvest_precision() {
            let scratch = scratch();
            let queue = open(&scratch);
            let transport = writable_account();

            let started_at = ago(3600.0);
            queue.enqueue(create(
                Uuid::new_v4(),
                started_at,
                Some(offset_by(started_at, 456.0)),
            ));
            queue.enqueue(Mutation::Stop {
                id: EntryId::Server(636709355),
                hours: 2.8e-05,
            });
            queue.drain(&client(transport.clone())).await;

            assert_eq!(
                transport.calls_to(Method::Post, "time_entries")[0].hours,
                Some(0.13)
            );
            assert_eq!(correction(&transport).hours, Some(0.0));
        }

        /// A timer started and stopped before the queue ever drained should reach
        /// Harvest as one finished entry, not a create followed by a stop.
        #[test]
        fn amends_queued_create() {
            let scratch = scratch();
            let queue = open(&scratch);
            let local = Uuid::new_v4();

            queue.enqueue(create(local, Timestamp::now(), None));
            assert!(queue.amend_create(local, Some(Timestamp::now())));
            assert_eq!(queue.count(), 1);
            assert!(!queue.amend_create(Uuid::new_v4(), Some(Timestamp::now())));
        }
    }

    mod local_identifier_handoff {
        use super::*;

        fn writable_account() -> Arc<RoutingTransport> {
            RoutingTransport::new(vec![
                Route::new(Method::Patch, "/stop", &fixture::time_entry()),
                Route::new(Method::Patch, "/restart", &fixture::running_time_entry()),
                Route::new(Method::Post, "time_entries", &fixture::running_time_entry())
                    .status(201),
                Route::new(Method::Patch, "time_entries", &fixture::time_entry()),
            ])
        }

        /// Stopping a timer after its create drained, but before the tracker
        /// adopted the real id, used to be discarded as "not found", leaving the
        /// timer running on Harvest forever.
        #[tokio::test]
        async fn resolves_local_id_after_create() {
            let scratch = scratch();
            let queue = open(&scratch);
            let transport = writable_account();
            let client = client(transport.clone());
            let local = Uuid::new_v4();

            queue.enqueue(create(local, ago(900.0), Some(Timestamp::now())));
            let first = queue.drain(&client).await;
            assert_eq!(first.resolved.get(&local), Some(&636708906));

            // Queued only now, after the create drained, so no rewrite could have run.
            queue.enqueue(Mutation::Stop {
                id: EntryId::Local(local),
                hours: 0.25,
            });
            let second = queue.drain(&client).await;

            assert_eq!(second.applied, 1);
            assert!(second.discarded.is_empty());
            assert_eq!(transport.calls_to(Method::Patch, "636708906/stop").len(), 1);
        }

        #[tokio::test]
        async fn resolutions_survive_relaunch() {
            let scratch = scratch();
            let transport = writable_account();
            let client = client(transport);
            let local = Uuid::new_v4();

            let first = open(&scratch);
            first.enqueue(create(local, ago(900.0), Some(Timestamp::now())));
            first.drain(&client).await;

            let reopened = open(&scratch);
            reopened.enqueue(Mutation::Stop {
                id: EntryId::Local(local),
                hours: 0.25,
            });
            let report = reopened.drain(&client).await;

            assert_eq!(report.applied, 1);
            assert!(report.discarded.is_empty());
        }
    }

    /// The queue must never send the same create twice. A create that needs a
    /// second request, a restart or a stop, commits to the queue the moment its POST
    /// succeeds, and the second request takes its place as an ordinary mutation; a
    /// failure between the two retries only the second.
    mod create_follow_ups {
        use super::*;

        #[tokio::test]
        async fn restart_failure_does_not_repeat_create() {
            let scratch = scratch();
            let queue = open(&scratch);
            queue.enqueue(create(Uuid::new_v4(), ago(3600.0), None));

            // The create lands; the restart cannot.
            let failing = RoutingTransport::new(vec![
                Route::new(Method::Post, "time_entries", &fixture::time_entry()).status(201),
                Route::new(Method::Patch, "/restart", "{}").status(500),
            ]);
            let first = queue.drain(&client(failing.clone())).await;
            assert_eq!(first.applied, 1);
            assert!(first.stopped_with.is_some());
            assert_eq!(failing.calls_to(Method::Post, "time_entries").len(), 1);

            // Reopened from disk, as after a relaunch: only the restart goes out.
            let healthy = RoutingTransport::new(vec![
                Route::new(Method::Patch, "/restart", &fixture::running_time_entry()),
                Route::new(Method::Patch, "time_entries", &fixture::time_entry()),
            ]);
            let reopened = open(&scratch);
            let second = reopened.drain(&client(healthy.clone())).await;
            assert_eq!(second.applied, 1);
            assert!(second.discarded.is_empty());
            assert!(healthy.calls_to(Method::Post, "time_entries").is_empty());
            assert_eq!(healthy.calls_to(Method::Patch, "/restart").len(), 1);
            assert!(reopened.is_empty());
        }

        #[tokio::test]
        async fn stop_failure_does_not_repeat_create() {
            let scratch = scratch();
            let queue = open(&scratch);
            let started_at = ago(5.0);
            queue.enqueue(create(
                Uuid::new_v4(),
                started_at,
                Some(offset_by(started_at, 5.0)),
            ));

            let failing = RoutingTransport::new(vec![
                Route::new(Method::Post, "time_entries", &fixture::running_time_entry())
                    .status(201),
                Route::new(Method::Patch, "/stop", "{}").status(500),
            ]);
            let first = queue.drain(&client(failing)).await;
            assert_eq!(first.applied, 1);
            assert!(first.stopped_with.is_some());

            let healthy = RoutingTransport::new(vec![
                Route::new(Method::Patch, "/stop", &fixture::time_entry()),
                Route::new(Method::Patch, "time_entries", &fixture::time_entry()),
            ]);
            let second = queue.drain(&client(healthy.clone())).await;
            assert_eq!(second.applied, 1);
            assert!(healthy.calls_to(Method::Post, "time_entries").is_empty());
            assert_eq!(healthy.calls_to(Method::Patch, "/stop").len(), 1);
            assert!(queue.is_empty());
        }
    }

    /// Typing a total onto a running timer while the network is down must not
    /// freeze the clock at the typed number: Harvest counts on from the moment the
    /// patch arrives, so the queue folds in the time since the user chose it.
    mod running_adjustment {
        use super::*;

        #[tokio::test]
        async fn adjust_adds_elapsed_since_chosen() {
            let scratch = scratch();
            let queue = open(&scratch);
            let transport = RoutingTransport::new(vec![Route::new(
                Method::Patch,
                "time_entries",
                &fixture::running_time_entry(),
            )]);

            // "1" typed half an hour ago: Harvest must hear 1:30.
            queue.enqueue(Mutation::Adjust {
                id: EntryId::Server(636708906),
                hours: 1.0,
                as_of: ago(1800.0),
            });
            let report = queue.drain(&client(transport.clone())).await;

            assert_eq!(report.applied, 1);
            let hours = transport.calls_to(Method::Patch, "time_entries")[0]
                .hours
                .unwrap();
            assert!((hours - 1.5).abs() < 0.01);
        }
    }
}
