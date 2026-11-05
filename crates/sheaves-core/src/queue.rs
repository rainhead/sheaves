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
use crate::peers::board;
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
    ///
    /// `run`, when present, is what the local copy believed about the timer, so
    /// delivery can check it against Harvest first: another machine may have
    /// trimmed or stopped the timer since. Without it, `hours` is sent as it is,
    /// which is right for a stop that follows this queue's own create, and is how
    /// stops queued before peers existed are read.
    Stop {
        id: EntryId,
        hours: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run: Option<StoppedRun>,
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
        /// A new project and task for the entry. Absent from changes queued
        /// before entries could be moved.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<TimerTarget>,
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

/// A timer's run as the local copy saw it when the user stopped it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoppedRun {
    /// When the local copy believed the timer started.
    pub started_at: Timestamp,
    /// Hours the local copy believed were banked before that start.
    pub banked_hours: f64,
    /// When the user stopped it.
    pub stopped_at: Timestamp,
}

impl StoppedRun {
    /// Allowance for Harvest's clock and this machine's to disagree, as
    /// [`Absence::START_TOLERANCE`](crate::absence::Absence::START_TOLERANCE)
    /// allows between a click and the timer it started.
    pub const CLOCK_SLACK: f64 = 120.0;

    /// The hours to send for this stop, judged against `entry` as Harvest holds it
    /// now, or `None` when the stop no longer applies and is to be dropped.
    ///
    /// - Not running: another client stopped it, and its hours stand.
    /// - Running since when the local copy thought: nothing has changed, and the
    ///   hours measured locally go.
    /// - Running since some other moment: something restarted it since, such as a
    ///   trim on another machine, or this queue's own restart landing late. Either
    ///   way Harvest's banked hours cover everything before that start, so the hours
    ///   are Harvest's banked hours plus the time from that start to the stop.
    /// - Running since well after the user stopped: the timer was started again
    ///   after the moment this stop speaks for, and the later action wins.
    pub fn hours_against(&self, entry: &TimeEntry, measured: f64) -> Option<f64> {
        if !entry.is_running {
            return None;
        }
        let Some(started) = entry.timer_started_at else {
            return Some(measured);
        };
        let banked = entry.hours_without_timer.unwrap_or(0.0);
        // Harvest keeps hours to two decimals; anything closer is the same figure.
        if started == self.started_at && (banked - self.banked_hours).abs() < 0.01 {
            return Some(measured);
        }
        if started > crate::models::offset_by(self.stopped_at, Self::CLOCK_SLACK) {
            return None;
        }
        Some((banked + crate::models::seconds_between(started, self.stopped_at) / 3600.0).max(0.0))
    }
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

    /// What the change is and which entry it acts on, for a log line. Never the
    /// notes or anything else the user wrote, which the log must not carry.
    pub fn summary(&self) -> String {
        let kind = match self {
            Mutation::Create { .. } => "create",
            Mutation::Stop { .. } => "stop",
            Mutation::Restart { .. } => "restart",
            Mutation::Update { .. } => "update",
            Mutation::Adjust { .. } => "adjust",
            Mutation::Delete { .. } => "delete",
        };
        match self.subject() {
            EntryId::Server(id) => format!("{kind} of entry {id}"),
            EntryId::Local(local) => format!("{kind} of unsent entry {local}"),
        }
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
    /// Harvest ids of the entries this drain changed, so other machines can be told.
    pub touched: Vec<i64>,
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

/// A mutation as the queue holds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Queued {
    /// Minted when the change is enqueued, and how the drain recognises the change
    /// it sent. Comparing whole values instead broke whenever anything amended a
    /// queued change while it was on its way.
    id: Uuid,
    mutation: Mutation,
    /// When a create was last sent. Set before the request goes, so that after a
    /// failure that leaves it unclear whether Harvest saved the entry, the next
    /// attempt looks for the entry before posting a second one.
    #[serde(default)]
    attempted_at: Option<Timestamp>,
}

impl Queued {
    fn new(mutation: Mutation) -> Self {
        Self {
            id: Uuid::new_v4(),
            mutation,
            attempted_at: None,
        }
    }
}

/// A queued change as some build wrote it: with an id, or, from the phase 2 build
/// that came before ids, the bare change, which is given one on reading.
#[derive(Deserialize)]
#[serde(untagged)]
enum StoredChange {
    Queued(Queued),
    Bare(Mutation),
}

fn read_pending<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<Queued>, D::Error> {
    let changes = Vec::<StoredChange>::deserialize(deserializer)?;
    Ok(changes
        .into_iter()
        .map(|change| match change {
            StoredChange::Queued(queued) => queued,
            StoredChange::Bare(mutation) => Queued::new(mutation),
        })
        .collect())
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default, deserialize_with = "read_pending")]
    pending: Vec<Queued>,
    /// Local ids that have earned a Harvest id, newest last: the one place a change
    /// queued against a local id learns which entry it means. A change can be queued
    /// after its create drained but before the tracker adopted the real id, so the
    /// mapping has to outlive the create.
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
        self.state
            .lock()
            .unwrap()
            .pending
            .iter()
            .map(|queued| queued.mutation.clone())
            .collect()
    }

    pub fn enqueue(&self, mutation: Mutation) {
        self.change(|state| state.pending.push(Queued::new(mutation)));
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

    /// Makes the queue `owner`'s if nobody's work is in it: it has no owner, or it
    /// is empty. Checked and claimed under one lock, so a change enqueued meanwhile
    /// cannot be handed to the wrong person.
    ///
    /// An empty queue can keep a stale owner when the app dies between saving new
    /// credentials and claiming the queue for them; left so, the new user's next
    /// change would be refused as someone else's.
    pub fn claim_if_free(&self, owner: Owner) {
        self.change(|state| {
            if state.owner.as_ref() == Some(&owner) {
                return;
            }
            if state.owner.is_none() || state.pending.is_empty() {
                state.resolved.clear();
                state.owner = Some(owner);
            }
        });
    }

    /// Marks a queued create as finished without adding a second mutation, for a
    /// timer started and stopped before the queue ever drained. Returns false when
    /// the create cannot be amended, and the caller queues a stop or a restart
    /// instead, which carry the times the tracker measured.
    ///
    /// A create's hours are the span from its start to its end, so it can describe
    /// only a timer that ran once without a break. It is refused:
    ///
    /// - when it is not pending;
    /// - when it is the head of a queue that is draining, since it may already be on
    ///   its way to Harvest. Amended in flight, it would no longer match the copy
    ///   the drain sent, so the drain would leave it queued and send it again: a
    ///   second entry for one timer;
    /// - when it has been attempted, since Harvest may hold it already, with the
    ///   hours measured at that attempt, and an end amended here would be lost when
    ///   the retry adopts that entry;
    /// - when it has ended and the caller would reopen it, or when a later queued
    ///   change already refers to it. Either way the timer has paused, and a span
    ///   from the first start would bill the pause.
    pub fn amend_create(&self, local: Uuid, ended_at: Option<Timestamp>) -> bool {
        let mut amended = false;
        self.change(|state| {
            let Some(index) = state.pending.iter().position(|queued| {
                matches!(queued.mutation, Mutation::Create { local: id, .. } if id == local)
            }) else {
                return;
            };
            // Read under the lock the drain takes to copy the head, so the two
            // cannot interleave.
            if index == 0 && self.is_draining.load(Ordering::SeqCst) {
                return;
            }
            if state.pending[index].attempted_at.is_some() {
                return;
            }
            if state.pending[index + 1..]
                .iter()
                .any(|queued| queued.mutation.subject() == EntryId::Local(local))
            {
                return;
            }
            let Mutation::Create { ended_at: end, .. } = &mut state.pending[index].mutation else {
                unreachable!("found as a create");
            };
            if end.is_some() && ended_at.is_none() {
                return;
            }
            *end = ended_at;
            amended = true;
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
            let Some(head) = self.state.lock().unwrap().pending.first().cloned() else {
                break;
            };
            let mutation = head.mutation.clone();
            let outcome = match &mutation {
                Mutation::Create { local, .. } => {
                    // A create can need a second request, a restart or a stop.
                    // Sending both inside one mutation meant a transient failure of
                    // the second repeated the first: one duplicate entry per retry,
                    // with the original left running. So the create is committed to
                    // the queue the moment the POST succeeds, and the second request
                    // takes its place as an ordinary mutation with ordinary retries.
                    let local = *local;
                    match self.deliver_create(&head, client).await {
                        Ok((entry, follow_up)) => {
                            self.change(|state| {
                                remember(&mut state.resolved, local, entry.id);
                                // The queue can have been emptied while the request
                                // was in flight, by `remove_all` on disconnect.
                                // Touch it only where the drained change stands.
                                if state.pending.first().map(|queued| queued.id) == Some(head.id) {
                                    match follow_up {
                                        Some(follow_up) => {
                                            state.pending[0] = Queued::new(follow_up)
                                        }
                                        None => {
                                            state.pending.remove(0);
                                        }
                                    }
                                }
                            });
                            report.resolved.insert(local, entry.id);
                            report.touched.push(entry.id);
                            Ok(())
                        }
                        Err(error) => Err(error),
                    }
                }
                Mutation::Stop {
                    id,
                    hours,
                    run: Some(run),
                } => match self.deliver_stop(&head, *id, *hours, run, client).await {
                    Ok(()) => {
                        self.remove_head(head.id);
                        report.touched.extend(self.server_id(*id));
                        Ok(())
                    }
                    Err(error) => Err(error),
                },
                _ => match self.apply(&mutation, client).await {
                    Ok(()) => {
                        self.remove_head(head.id);
                        report.touched.extend(self.server_id(mutation.subject()));
                        Ok(())
                    }
                    Err(error) => Err(error),
                },
            };
            match outcome {
                Ok(()) => report.applied += 1,
                // A rejected token fails every request alike. Treated as a refusal
                // of this one change, it would discard the whole queue in one drain,
                // and the work a renewed token exists to send would be gone. A create
                // whose reply would not decode may well have been saved; it waits
                // for the next drain, which looks for it before posting again.
                Err(error)
                    if error.is_transient()
                        || matches!(
                            error,
                            HarvestError::Unauthorized | HarvestError::NotConfigured
                        )
                        || (matches!(mutation, Mutation::Create { .. })
                            && matches!(error, HarvestError::Decoding { .. })) =>
                {
                    report.stopped_with = Some(error);
                    break;
                }
                Err(error) => {
                    self.remove_head(head.id);
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

    fn remove_head(&self, id: Uuid) {
        self.change(|state| {
            if state.pending.first().map(|queued| queued.id) == Some(id) {
                state.pending.remove(0);
            }
        });
    }

    /// Delivers the create at the head of the queue, once.
    ///
    /// A create sent before, whose fate is unknown because its request failed in a
    /// way that leaves it unclear whether Harvest saved the entry, is looked for
    /// first. Only when Harvest has no such entry is it posted again.
    async fn deliver_create(
        &self,
        head: &Queued,
        client: &HarvestClient,
    ) -> Result<(TimeEntry, Option<Mutation>), HarvestError> {
        let found = match head.attempted_at {
            Some(attempted_at) => self
                .find_created(&head.mutation, attempted_at, client)
                .await
                // The lookup failing says nothing about the create. Taken as its
                // refusal, the drain would discard the user's change; it waits for
                // the next drain instead, unless the token itself was refused.
                .map_err(|error| match error {
                    HarvestError::Unauthorized | HarvestError::NotConfigured => error,
                    other => HarvestError::Network {
                        reason: format!(
                            "could not check whether an earlier attempt was saved: {other}"
                        ),
                    },
                })?,
            None => None,
        };
        if let Some(entry) = found {
            log::info!(
                "found entry {} from an earlier attempt; not posting again",
                entry.id
            );
            let follow_up = Self::follow_up_for_found(&head.mutation, &entry);
            return Ok((entry, follow_up));
        }
        let now = self.clock.now();
        self.change(|state| {
            if let Some(first) = state.pending.first_mut()
                && first.id == head.id
            {
                first.attempted_at = Some(now);
            }
        });
        self.create(&head.mutation, client).await
    }

    /// Delivers a stop that knows what the local copy believed, after reading the
    /// entry back from Harvest. See [`StoppedRun::hours_against`] for the rule.
    ///
    /// The hours decided on are written into the queued stop before `/stop` is
    /// sent, with the attempt. If `/stop` lands and its reply or the hours
    /// correction is lost, the retry finds the timer stopped, and the attempt says
    /// whose stop that was: this queue's, so the hours it decided on still go.
    async fn deliver_stop(
        &self,
        head: &Queued,
        id: EntryId,
        measured: f64,
        run: &StoppedRun,
        client: &HarvestClient,
    ) -> Result<(), HarvestError> {
        let server_id = self.server_id(id).ok_or(HarvestError::NotFound)?;
        let hours = match client.time_entry(server_id).await {
            Ok(entry) if !entry.is_running && head.attempted_at.is_some() => measured,
            Ok(entry) => match run.hours_against(&entry, measured) {
                Some(hours) => hours,
                None => {
                    log::info!(
                        "not sending the stop of entry {server_id}: another client stopped or restarted it since"
                    );
                    return Ok(());
                }
            },
            // An entry that will not decode cannot be judged, and the stop as
            // measured is what this queue sent before it could re-read anything.
            Err(HarvestError::Decoding { endpoint, detail }) => {
                log::error!("{endpoint} did not decode, so the stop goes as measured: {detail}");
                measured
            }
            Err(error) => return Err(error),
        };
        let now = self.clock.now();
        self.change(|state| {
            if let Some(first) = state.pending.first_mut()
                && first.id == head.id
            {
                first.attempted_at = Some(now);
                if let Mutation::Stop { hours: queued, .. } = &mut first.mutation {
                    *queued = hours;
                }
            }
        });
        self.apply(
            &Mutation::Stop {
                id,
                hours,
                run: None,
            },
            client,
        )
        .await
    }

    /// Harvest has no idempotency key, so an earlier attempt is recognised by what it
    /// sent: the same project, task, day and notes, on an entry created since the
    /// attempt and not already taken by another create. Harvest's clock and this
    /// machine's can differ, so the search reaches back a few minutes further.
    async fn find_created(
        &self,
        mutation: &Mutation,
        attempted_at: Timestamp,
        client: &HarvestClient,
    ) -> Result<Option<TimeEntry>, HarvestError> {
        const CLOCK_SLACK: f64 = 5.0 * 60.0;
        let Mutation::Create {
            target,
            spent_date,
            notes,
            ..
        } = mutation
        else {
            return Ok(None);
        };
        // The tracker claims a queue before draining it, so this is a fallback: with
        // no owner there is no telling whose entries to search, and posting again
        // could duplicate the entry.
        let Some(owner) = self.owner() else {
            return Err(HarvestError::Network {
                reason: "the queue has no owner yet, so an earlier attempt cannot be checked"
                    .to_string(),
            });
        };
        let taken: Vec<i64> = {
            let state = self.state.lock().unwrap();
            state
                .resolved
                .iter()
                .map(|resolution| resolution.server_id)
                .collect()
        };
        let since = crate::models::offset_by(attempted_at, -CLOCK_SLACK);
        let entries = client
            .time_entries(owner.user_id, *spent_date, *spent_date)
            .await?;
        Ok(entries.into_iter().find(|entry| {
            entry.project.id == target.project.id
                && entry.task.id == target.task.id
                && entry.notes.as_deref().unwrap_or("") == notes.as_deref().unwrap_or("")
                && entry.created_at >= since
                && !taken.contains(&entry.id)
        }))
    }

    /// What must follow a create that an earlier attempt already made, judged from
    /// the entry Harvest holds.
    fn follow_up_for_found(mutation: &Mutation, entry: &TimeEntry) -> Option<Mutation> {
        let Mutation::Create {
            started_at,
            ended_at,
            ..
        } = mutation
        else {
            return None;
        };
        let id = EntryId::Server(entry.id);
        match (ended_at, entry.is_running) {
            (Some(ended_at), true) => Some(Mutation::Stop {
                id,
                hours: Self::storable_hours(hours_between(*started_at, *ended_at)),
                run: None,
            }),
            (None, false) => Some(Mutation::Restart {
                id,
                resumed_at: entry.created_at,
                banked_hours: entry.hours,
            }),
            _ => None,
        }
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
                run: None,
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
                match landed(client.stop_time_entry(server_id).await) {
                    // Already stopped on the server: the hours still need correcting.
                    Ok(()) | Err(HarvestError::Rejected { .. }) => {}
                    Err(error) => return Err(error),
                }
                let stopped = client
                    .update_time_entry(server_id, hours_only(Self::storable_hours(*hours)))
                    .await;
                let shared = matches!(
                    &stopped,
                    Ok(TimeEntry { external_reference: Some(reference), .. })
                        if reference.group_id.as_deref() == Some(board::GROUP)
                );
                landed(stopped)?;
                // A finished entry carries nothing of presence: the stretches were
                // only ever about the timer while it ran. The stop has landed, so
                // a failure here costs only a stale icon on the entry.
                if shared && let Err(error) = client.remove_external_reference(server_id).await {
                    log::info!("could not clear what was shared on entry {server_id}: {error}");
                }
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
                    landed(
                        client
                            .update_time_entry(
                                server_id,
                                hours_only(Self::storable_hours(banked_hours + offline)),
                            )
                            .await,
                    )?;
                }
                landed(client.restart_time_entry(server_id).await)?;
            }
            Mutation::Update {
                notes,
                hours,
                target,
                ..
            } => {
                landed(
                    client
                        .update_time_entry(
                            server_id,
                            EntryUpdate {
                                notes: notes.as_deref(),
                                hours: hours.map(Self::storable_hours),
                                project_id: target.as_ref().map(|target| target.project.id),
                                task_id: target.as_ref().map(|target| target.task.id),
                                ..EntryUpdate::default()
                            },
                        )
                        .await,
                )?;
            }
            Mutation::Adjust { hours, as_of, .. } => {
                let elapsed = hours_between(*as_of, now);
                landed(
                    client
                        .update_time_entry(
                            server_id,
                            hours_only(Self::storable_hours(hours + elapsed)),
                        )
                        .await,
                )?;
            }
            Mutation::Delete { .. } => client.delete_time_entry(server_id).await?,
        }
        Ok(())
    }

    /// The Harvest id a local id earned, if this queue has seen it created.
    /// The Harvest id `id` stands for: its own, or the one its create earned.
    pub fn server_id(&self, id: EntryId) -> Option<i64> {
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

/// A write's outcome, with a reply that would not decode counted as the write
/// having landed: Harvest answered with success, and only reading the answer failed.
/// Counted as a failure, it dropped a stop before its hours correction was sent.
fn landed<T>(result: Result<T, HarvestError>) -> Result<(), HarvestError> {
    match result {
        Ok(_) => Ok(()),
        Err(HarvestError::Decoding { endpoint, detail }) => {
            log::error!("{endpoint} landed, but its reply did not decode: {detail}");
            Ok(())
        }
        Err(error) => Err(error),
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
                target: None,
            });
            queue.enqueue(Mutation::Stop {
                id: EntryId::Server(636709355),
                hours: 1.0,
                run: None,
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
                run: None,
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
                run: None,
            });
            queue.enqueue(Mutation::Stop {
                id: EntryId::Server(2),
                hours: 1.0,
                run: None,
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
                target: None,
            });
            queue.enqueue(Mutation::Stop {
                id: EntryId::Server(2),
                hours: 1.0,
                run: None,
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
                run: None,
            });
            queue.enqueue(Mutation::Stop {
                id: EntryId::Server(2),
                hours: 1.0,
                run: None,
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
                run: None,
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
        fn claims_only_a_queue_with_nobody_s_work() {
            let scratch = scratch();
            let queue = open(&scratch);
            let first = Owner {
                account_id: "1".to_string(),
                user_id: 1,
            };
            let second = Owner {
                account_id: "1".to_string(),
                user_id: 2,
            };

            queue.claim_if_free(first.clone());
            assert_eq!(queue.owner(), Some(first.clone()));

            queue.enqueue(Mutation::Delete {
                id: EntryId::Server(1),
            });
            queue.claim_if_free(second.clone());
            assert_eq!(queue.owner(), Some(first), "someone's work is queued");

            queue.remove_all();
            queue.claim(Owner {
                account_id: "9".to_string(),
                user_id: 9,
            });
            queue.claim_if_free(second.clone());
            assert_eq!(queue.owner(), Some(second), "an empty queue is nobody's");
        }

        /// The log is readable by anyone at the machine, so it gets what a change
        /// is and which entry, never what the user wrote.
        #[test]
        fn summarises_without_the_notes() {
            let update = Mutation::Update {
                id: EntryId::Server(42),
                notes: Some("Client X merger talks".to_string()),
                hours: None,
                target: None,
            };
            assert_eq!(update.summary(), "update of entry 42");
        }

        /// The build before ids wrote bare changes; they are read, not set aside.
        #[test]
        fn reads_a_queue_written_before_ids() {
            let scratch = scratch();
            std::fs::write(
                &scratch.path,
                r#"{"pending":[{"kind":"stop","id":{"server":636709355},"hours":1.0}],"resolved":[],"owner":null}"#,
            )
            .unwrap();

            let queue = open(&scratch);

            assert_eq!(
                queue.pending(),
                [Mutation::Stop {
                    id: EntryId::Server(636709355),
                    hours: 1.0,
                    run: None,
                }]
            );
        }

        #[test]
        fn persists_across_launches() {
            let scratch = scratch();
            let first = open(&scratch);
            first.enqueue(Mutation::Stop {
                id: EntryId::Server(636709355),
                hours: 1.0,
                run: None,
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
                run: None,
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
                run: None,
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

        /// Stopped and resumed before the queue drained: the create cannot describe
        /// that, since its span from the first start would bill the pause. Reopening
        /// is refused, the tracker queues a restart from the resume, and from then on
        /// the create stays as it is.
        #[test]
        fn never_reopens_a_finished_create() {
            let scratch = scratch();
            let queue = open(&scratch);
            let local = Uuid::new_v4();
            queue.enqueue(create(local, ago(3600.0), None));
            assert!(queue.amend_create(local, Some(ago(1800.0))));

            assert!(!queue.amend_create(local, None));
            queue.enqueue(Mutation::Restart {
                id: EntryId::Local(local),
                resumed_at: ago(600.0),
                banked_hours: 0.5,
            });
            assert!(!queue.amend_create(local, Some(Timestamp::now())));

            let Mutation::Create { ended_at, .. } = &queue.pending()[0] else {
                panic!("the create is still first");
            };
            assert!(ended_at.is_some());
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
                run: None,
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
                run: None,
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

    /// A create whose request failed in a way that leaves it unclear whether Harvest
    /// saved it, a 5xx or a timeout after the body went, must not be posted a second
    /// time if Harvest did. Harvest has no idempotency key, so the retry looks first.
    mod ambiguous_failures {
        use super::*;

        fn owned(scratch: &Scratch) -> MutationQueue {
            let queue = open(scratch);
            queue.claim(Owner {
                account_id: "12345".to_string(),
                user_id: 1782959,
            });
            queue
        }

        /// The entry an attempt might have saved: the target's project and task, no
        /// notes, created just now.
        fn saved_entry(id: i64) -> String {
            fixture::time_entry()
                .replace("636709355", &id.to_string())
                .replace("14307913", "14308069")
                .replace("8083365", "8083366")
                .replace(r#""Adding CSS styling""#, "null")
                .replace("2026-08-29T22:44:36Z", &Timestamp::now().to_string())
        }

        fn first_attempt_fails(queue: &MutationQueue) -> impl Future<Output = DrainReport> {
            let failing = RoutingTransport::new(vec![
                Route::new(Method::Post, "time_entries", "").status(500),
            ]);
            let client = client(failing);
            async move { queue.drain(&client).await }
        }

        #[tokio::test]
        async fn a_retry_adopts_the_entry_the_first_attempt_saved() {
            let scratch = scratch();
            let queue = owned(&scratch);
            let local = Uuid::new_v4();
            queue.enqueue(create(local, ago(3600.0), Some(Timestamp::now())));
            assert!(first_attempt_fails(&queue).await.stopped_with.is_some());

            let retry = RoutingTransport::new(vec![
                Route::new(
                    Method::Get,
                    "time_entries",
                    &fixture::entries_page(&[saved_entry(777)]),
                ),
                Route::new(Method::Post, "time_entries", &fixture::time_entry()).status(201),
            ]);
            let report = queue.drain(&client(retry.clone())).await;

            assert!(retry.calls_to(Method::Post, "time_entries").is_empty());
            assert_eq!(report.resolved.get(&local), Some(&777));
            assert!(queue.is_empty());
        }

        #[tokio::test]
        async fn a_retry_posts_again_when_nothing_was_saved() {
            let scratch = scratch();
            let queue = owned(&scratch);
            queue.enqueue(create(Uuid::new_v4(), ago(3600.0), Some(Timestamp::now())));
            assert!(first_attempt_fails(&queue).await.stopped_with.is_some());

            let retry = RoutingTransport::new(vec![
                Route::new(Method::Get, "time_entries", &fixture::entries_page(&[])),
                Route::new(Method::Post, "time_entries", &fixture::time_entry()).status(201),
            ]);
            queue.drain(&client(retry.clone())).await;

            assert_eq!(retry.calls_to(Method::Post, "time_entries").len(), 1);
            assert!(queue.is_empty());
        }

        /// An entry that only looks alike, because it was created before this
        /// attempt, is somebody's earlier work and must not be taken.
        #[tokio::test]
        async fn an_older_lookalike_is_not_taken() {
            let scratch = scratch();
            let queue = owned(&scratch);
            queue.enqueue(create(Uuid::new_v4(), ago(3600.0), Some(Timestamp::now())));
            assert!(first_attempt_fails(&queue).await.stopped_with.is_some());

            let older =
                saved_entry(778).replace(&Timestamp::now().to_string()[..13], "2020-01-01T00");
            let retry = RoutingTransport::new(vec![
                Route::new(
                    Method::Get,
                    "time_entries",
                    &fixture::entries_page(&[older]),
                ),
                Route::new(Method::Post, "time_entries", &fixture::time_entry()).status(201),
            ]);
            queue.drain(&client(retry.clone())).await;

            assert_eq!(retry.calls_to(Method::Post, "time_entries").len(), 1);
        }

        /// A create whose reply would not decode was very likely saved. It stays
        /// queued, and the next drain finds it rather than posting again.
        #[tokio::test]
        async fn an_unreadable_reply_is_not_taken_for_a_refusal() {
            let scratch = scratch();
            let queue = owned(&scratch);
            let local = Uuid::new_v4();
            queue.enqueue(create(local, ago(3600.0), Some(Timestamp::now())));
            let garbled = RoutingTransport::new(vec![
                Route::new(Method::Post, "time_entries", "{}").status(201),
            ]);
            let first = queue.drain(&client(garbled)).await;
            assert!(first.discarded.is_empty());
            assert_eq!(queue.count(), 1);

            let retry = RoutingTransport::new(vec![Route::new(
                Method::Get,
                "time_entries",
                &fixture::entries_page(&[saved_entry(779)]),
            )]);
            let second = queue.drain(&client(retry)).await;
            assert_eq!(second.resolved.get(&local), Some(&779));
            assert!(queue.is_empty());
        }

        /// A create that may already be on Harvest is not amended: the retry would
        /// adopt the saved entry and the amended end would be lost with it.
        #[tokio::test]
        async fn an_attempted_create_is_not_amended() {
            let scratch = scratch();
            let queue = owned(&scratch);
            let local = Uuid::new_v4();
            queue.enqueue(create(local, ago(3600.0), None));
            assert!(first_attempt_fails(&queue).await.stopped_with.is_some());

            assert!(!queue.amend_create(local, Some(Timestamp::now())));
        }

        /// The lookup failing is not the create failing: the change stays queued.
        #[tokio::test]
        async fn a_failed_lookup_keeps_the_create() {
            let scratch = scratch();
            let queue = owned(&scratch);
            queue.enqueue(create(Uuid::new_v4(), ago(3600.0), Some(Timestamp::now())));
            assert!(first_attempt_fails(&queue).await.stopped_with.is_some());

            let broken = RoutingTransport::new(vec![
                Route::new(Method::Get, "time_entries", "{}").status(404),
            ]);
            let report = queue.drain(&client(broken)).await;

            assert!(report.discarded.is_empty());
            assert!(report.stopped_with.is_some());
            assert_eq!(queue.count(), 1);
        }

        /// A stop that reached Harvest still needs its hours corrected, even when
        /// the reply to the stop could not be read.
        #[tokio::test]
        async fn a_stop_with_an_unreadable_reply_is_still_corrected() {
            let scratch = scratch();
            let queue = owned(&scratch);
            let transport = RoutingTransport::new(vec![
                Route::new(Method::Patch, "/stop", "{}"),
                Route::new(Method::Patch, "time_entries", &fixture::time_entry()),
            ]);
            queue.enqueue(Mutation::Stop {
                id: EntryId::Server(636709355),
                hours: 0.5,
                run: None,
            });

            let report = queue.drain(&client(transport.clone())).await;

            assert_eq!(report.applied, 1);
            assert!(report.discarded.is_empty());
            assert_eq!(correction(&transport).hours, Some(0.5));
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

    /// A stop that re-reads the entry before sending its hours, so another
    /// machine's trim or stop is not overwritten.
    mod stop_reread {
        use super::*;

        fn at(text: &str) -> Timestamp {
            text.parse().unwrap()
        }

        /// The entry as Harvest holds it, running since `started` with `banked`
        /// hours before that, or stopped when `started` is `None`.
        fn harvest_entry(started: Option<&str>, banked: f64) -> String {
            let mut entry: serde_json::Value =
                serde_json::from_str(&fixture::running_time_entry()).unwrap();
            entry["is_running"] = started.is_some().into();
            entry["timer_started_at"] = started.map_or(serde_json::Value::Null, Into::into);
            entry["hours_without_timer"] = banked.into();
            entry["hours"] = banked.into();
            entry.to_string()
        }

        /// The timer started at nine and was stopped here at half past two.
        fn stop() -> Mutation {
            Mutation::Stop {
                id: EntryId::Server(636708906),
                hours: 5.5,
                run: Some(StoppedRun {
                    started_at: at("2026-08-29T09:00:00Z"),
                    banked_hours: 0.0,
                    stopped_at: at("2026-08-29T14:30:00Z"),
                }),
            }
        }

        fn hours_sent(transport: &StubTransport) -> Vec<f64> {
            (0..transport.request_count())
                .map(|index| transport.request(index))
                .filter(|request| request.method == Method::Patch)
                .filter_map(|request| request.json().get("hours").and_then(|h| h.as_f64()))
                .collect()
        }

        fn paths(transport: &StubTransport) -> Vec<String> {
            (0..transport.request_count())
                .map(|index| {
                    let request = transport.request(index);
                    format!("{} {}", request.method.as_str(), request.url.path())
                })
                .collect()
        }

        #[tokio::test]
        async fn sends_the_measured_hours_when_nothing_changed() {
            let scratch = scratch();
            let queue = open(&scratch);
            let transport = StubTransport::new(vec![
                Scripted::ok(&harvest_entry(Some("2026-08-29T09:00:00Z"), 0.0)),
                Scripted::ok(&fixture::time_entry()),
                Scripted::ok(&fixture::time_entry()),
            ]);
            queue.enqueue(stop());
            let report = queue.drain(&client(transport.clone())).await;

            assert_eq!(report.applied, 1);
            assert_eq!(
                paths(&transport),
                [
                    "GET /v2/time_entries/636708906",
                    "PATCH /v2/time_entries/636708906/stop",
                    "PATCH /v2/time_entries/636708906",
                ]
            );
            assert_eq!(hours_sent(&transport), [5.5]);
            assert_eq!(report.touched, [636708906]);
        }

        /// The worked example in docs/gnome-port.md: the timer started at nine, was
        /// trimmed on another machine at two to continue from twelve with an hour
        /// banked, and was stopped here at half past two before this machine had
        /// heard. One hour banked plus two and a half since twelve, not five and a
        /// half.
        #[tokio::test]
        async fn recomputes_after_a_trim_elsewhere() {
            let scratch = scratch();
            let queue = open(&scratch);
            let transport = StubTransport::new(vec![
                Scripted::ok(&harvest_entry(Some("2026-08-29T12:00:00Z"), 1.0)),
                Scripted::ok(&fixture::time_entry()),
                Scripted::ok(&fixture::time_entry()),
            ]);
            queue.enqueue(stop());
            queue.drain(&client(transport.clone())).await;

            assert_eq!(hours_sent(&transport), [3.5]);
        }

        /// Stopped elsewhere already: the hours there stand, and nothing is sent.
        #[tokio::test]
        async fn drops_a_stop_another_client_made_first() {
            let scratch = scratch();
            let queue = open(&scratch);
            let transport = StubTransport::new(vec![Scripted::ok(&harvest_entry(None, 4.25))]);
            queue.enqueue(stop());
            let report = queue.drain(&client(transport.clone())).await;

            assert!(queue.is_empty());
            assert!(report.discarded.is_empty());
            assert_eq!(paths(&transport), ["GET /v2/time_entries/636708906"]);
        }

        /// Stopped elsewhere and started again after the moment this stop speaks
        /// for: the later action wins, and the timer keeps running.
        #[tokio::test]
        async fn drops_a_stop_older_than_a_restart_elsewhere() {
            let scratch = scratch();
            let queue = open(&scratch);
            let transport = StubTransport::new(vec![Scripted::ok(&harvest_entry(
                Some("2026-08-29T14:45:00Z"),
                5.0,
            ))]);
            queue.enqueue(stop());
            queue.drain(&client(transport.clone())).await;

            assert!(queue.is_empty());
            assert_eq!(paths(&transport), ["GET /v2/time_entries/636708906"]);
        }

        /// `/stop` landed and the hours correction failed. The retry finds the timer
        /// stopped, by this queue's own hand, and still sends the hours it decided
        /// on rather than dropping them as somebody else's stop.
        #[tokio::test]
        async fn finishes_its_own_stop_after_a_failure() {
            let scratch = scratch();
            let queue = open(&scratch);
            let first = StubTransport::new(vec![
                Scripted::ok(&harvest_entry(Some("2026-08-29T12:00:00Z"), 1.0)),
                Scripted::ok(&fixture::time_entry()),
                Scripted::status(500, ""),
            ]);
            queue.enqueue(stop());
            let report = queue.drain(&client(first.clone())).await;
            assert!(report.stopped_with.is_some());
            assert_eq!(queue.count(), 1);

            // Reopened from its file, as after a relaunch.
            let queue = open(&scratch);
            let second = StubTransport::new(vec![
                Scripted::ok(&harvest_entry(None, 6.0)),
                Scripted::status(422, r#"{"message": "This time entry is not running."}"#),
                Scripted::ok(&fixture::time_entry()),
            ]);
            queue.drain(&client(second.clone())).await;

            assert!(queue.is_empty());
            assert_eq!(hours_sent(&second), [3.5]);
        }

        /// A stop queued before stops carried their run reads back without one, and
        /// sends its hours as it always did.
        #[test]
        fn reads_a_stop_without_its_run() {
            let mutation: Mutation =
                serde_json::from_str(r#"{"kind":"stop","id":{"server":636708906},"hours":2.0}"#)
                    .unwrap();
            assert_eq!(
                mutation,
                Mutation::Stop {
                    id: EntryId::Server(636708906),
                    hours: 2.0,
                    run: None
                }
            );
        }
    }
}
