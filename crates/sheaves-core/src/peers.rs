//! Sharing presence between one person's machines, through Harvest.
//!
//! Start a timer on the Mac at nine, work at Hostess from ten to twelve, come back
//! to the Mac: alone, the Mac sees two hours with nobody at it and offers to trim
//! them. It lacks only Hostess's evidence. So each machine keeps a log of when it
//! saw the user, writes the part of it since the timer started onto the running
//! entry, and reads the other machines' parts there before it reports an absence.
//!
//! The entry is the user's own, so whoever can write it is the user or an
//! administrator of their account, and nothing needs a network of its own. See
//! [`board`] for what is written, and `docs/gnome-port.md` (phase 9) for why.

pub mod board;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::absence::{Absence, PresenceMonitor, PresenceReading};
use crate::models::{ExternalReference, offset_by, seconds_between};
use board::Board;

/// How far back the presence log reaches. A day covers any absence worth asking
/// about; a timer left running longer than that is a different problem.
const LOG_SPAN: f64 = 24.0 * 3600.0;

/// A stretch during which a machine had evidence of the user, with no gap in it as
/// long as the absence threshold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct Interval {
    pub start: Timestamp,
    pub end: Timestamp,
}

/// When this machine saw the user over the last day.
#[derive(Clone, Debug)]
pub struct PresenceLog {
    threshold: f64,
    /// In order, none overlapping, and each separated from the next by at least
    /// the threshold.
    intervals: Vec<Interval>,
}

impl PresenceLog {
    pub fn new(threshold: f64) -> Self {
        Self {
            threshold,
            intervals: Vec::new(),
        }
    }

    /// Records evidence that the user was here at `when`.
    ///
    /// Evidence closer than the threshold to an interval joins it, because a gap
    /// that short is not an absence: someone reading. Evidence can arrive out of
    /// order, as a call is dated to the previous poll, so it is placed where it
    /// belongs rather than appended.
    /// Returns whether `when` opened an interval of its own, which is what a peer's
    /// question turns on: judged before anything a day old is dropped, so a drop
    /// in the same call cannot hide it.
    pub fn note(&mut self, when: Timestamp) -> bool {
        let before = self.intervals.len();
        let at = self
            .intervals
            .partition_point(|interval| interval.start <= when);
        self.intervals.insert(
            at,
            Interval {
                start: when,
                end: when,
            },
        );
        let mut merged: Vec<Interval> = Vec::with_capacity(self.intervals.len());
        for interval in self.intervals.drain(..) {
            match merged.last_mut() {
                Some(last) if seconds_between(last.end, interval.start) < self.threshold => {
                    last.end = last.end.max(interval.end);
                }
                _ => merged.push(interval),
            }
        }
        self.intervals = merged;
        let opened = self.intervals.len() > before;
        let newest = self.intervals.last().map_or(when, |last| last.end);
        let horizon = offset_by(newest, -LOG_SPAN);
        self.intervals.retain(|interval| interval.end >= horizon);
        opened
    }

    /// Adds `stretch` as presence, joined to whatever it touches or comes within
    /// the threshold of, as `note` joins evidence.
    pub fn add(&mut self, stretch: Interval) {
        if stretch.start > stretch.end {
            return;
        }
        self.intervals.push(stretch);
        self.intervals.sort_by_key(|interval| interval.start);
        let mut merged: Vec<Interval> = Vec::with_capacity(self.intervals.len());
        for interval in self.intervals.drain(..) {
            match merged.last_mut() {
                Some(last) if seconds_between(last.end, interval.start) < self.threshold => {
                    last.end = last.end.max(interval.end);
                }
                _ => merged.push(interval),
            }
        }
        self.intervals = merged;
    }

    /// The intervals held, oldest first.
    pub fn intervals(&self) -> &[Interval] {
        &self.intervals
    }

    /// Takes `intervals` as the log, as read back from its file and with anything
    /// noted before: in order, with any that are not, or end more than a day before
    /// `now`, dropped, and any closer than the threshold joined, as `note` keeps
    /// them. Joining by whole interval keeps a long stretch whole.
    ///
    /// Anything starting after `now` is dropped too, as from a clock that ran
    /// ahead. Kept, it would never expire, since `note` measures a day back from
    /// the newest evidence, and it would vouch for the user over any absence a
    /// peer asked about.
    pub fn restore(&mut self, mut intervals: Vec<Interval>, now: Timestamp) {
        let horizon = offset_by(now, -LOG_SPAN);
        intervals.retain(|interval| {
            interval.start <= interval.end && interval.end >= horizon && interval.start <= now
        });
        for interval in &mut intervals {
            interval.end = interval.end.min(now);
        }
        intervals.sort_by_key(|interval| interval.start);
        let mut merged: Vec<Interval> = Vec::with_capacity(intervals.len());
        for interval in intervals {
            match merged.last_mut() {
                Some(last) if seconds_between(last.end, interval.start) < self.threshold => {
                    last.end = last.end.max(interval.end);
                }
                _ => merged.push(interval),
            }
        }
        self.intervals = merged;
    }

    /// The intervals that reach `since` or later.
    pub fn since(&self, since: Timestamp) -> Vec<Interval> {
        self.intervals
            .iter()
            .filter(|interval| interval.end >= since)
            .copied()
            .collect()
    }
}

/// The parts of `absence` that no interval in `seen` covers, and that are long
/// enough to count as absences themselves.
///
/// Peers' intervals can arrive in any order and overlap one another; what matters
/// is only which moments somebody saw the user. In the case this exists for, the
/// Mac's absence from ten to past twelve is covered by Hostess's interval from ten
/// to twelve, and what is left is a few minutes, too short to ask about.
pub fn unwitnessed(absence: Absence, seen: &[Interval], threshold: f64) -> Vec<Absence> {
    let mut seen: Vec<Interval> = seen
        .iter()
        .filter(|interval| interval.end > absence.began && interval.start < absence.ended)
        .copied()
        .collect();
    seen.sort_by_key(|interval| interval.start);
    let mut gaps = Vec::new();
    let mut began = absence.began;
    for interval in seen {
        if interval.start > began {
            gaps.push(Absence {
                began,
                ended: interval.start,
            });
        }
        began = began.max(interval.end);
    }
    if absence.ended > began {
        gaps.push(Absence {
            began,
            ended: absence.ended,
        });
    }
    gaps.retain(|gap| gap.duration() >= threshold);
    gaps
}

// MARK: The running entry

/// The running timer as Harvest holds it, for what is written on it.
#[derive(Clone, Debug, PartialEq)]
pub struct Posted {
    pub entry_id: i64,
    /// When the timer last started: stretches before it are about another run.
    pub timer_started_at: Timestamp,
    pub reference: Option<ExternalReference>,
}

/// Where the machines' stretches are kept. Harvest's running entry in the apps;
/// tests share one in memory between two instances.
#[async_trait]
pub trait Noticeboard: Send + Sync {
    /// The user's running timer as it is now, or `None` when none is running.
    async fn running(&self) -> Result<Option<Posted>, String>;

    /// Writes `reference` onto entry `entry_id`.
    async fn post(&self, entry_id: i64, reference: &ExternalReference) -> Result<(), String>;

    /// Whether writing is wanted at all just now. Reading for a question is
    /// always wanted; writing once a minute is not, in Low Power Mode.
    fn may_post(&self) -> bool {
        true
    }
}

// MARK: Peers

/// This machine's id on the board, made the first time and kept in `path`.
///
/// Random rather than the host name, which would be written to Harvest, and is
/// often too long to leave room for the stretches. Four characters give over a
/// million ids, for the few machines one person has.
pub fn machine_id_in(path: &std::path::Path) -> String {
    #[derive(Serialize, Deserialize)]
    struct Saved {
        id: String,
    }
    if let Some(saved) = crate::persistence::read_json::<Saved>(path)
        && board::is_machine_id(&saved.id)
    {
        return saved.id;
    }
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let random = uuid::Uuid::new_v4().as_u128();
    let id: String = (0..4)
        .map(|place| DIGITS[((random >> (place * 8)) % 36) as usize] as char)
        .collect();
    if let Err(error) = crate::persistence::write_json(path, &Saved { id: id.clone() }) {
        log::error!("could not keep this machine's id, so it will have another next time: {error}");
    }
    id
}

/// How often a machine rewrites its part while it sees the user.
pub const POST_EVERY: f64 = 60.0;

/// How long a question waits to read the running entry before this machine
/// decides alone. A request to Harvest may take half a minute to fail, and polls
/// wait their turn behind the one asking.
pub const READ_WITHIN: std::time::Duration = std::time::Duration::from_secs(5);

/// This machine's side of the sharing: the log it writes from, and where it reads
/// the others'.
pub struct Peers {
    /// This machine's id on the board; see [`board::is_machine_id`].
    machine: String,
    board: Mutex<Option<Arc<dyn Noticeboard>>>,
    log: Mutex<PresenceLog>,
    user_id: Mutex<Option<i64>>,
    /// Where the log is kept between launches, and when it was last written.
    log_file: Mutex<Option<(PathBuf, Option<Timestamp>)>>,
    /// Whose presence the log records. Evidence is of whoever is at the machine,
    /// but the log is shared for one Harvest user, so a different user signing in
    /// starts it afresh.
    log_owner: Mutex<Option<i64>>,
    /// When this machine last wrote its part, or tried to.
    posted_at: Mutex<Option<Timestamp>>,
    posting: AtomicBool,
    /// Writes take turns: each reads the entry, adds this machine's part and
    /// writes it, and one landing in the middle of another would be undone by it.
    write_turn: tokio::sync::Mutex<()>,
}

/// The log as kept on disk, with whose it is.
#[derive(Serialize, Deserialize)]
struct SavedLog {
    user_id: Option<i64>,
    intervals: Vec<Interval>,
}

/// How often the log is written to disk while the user is present: a machine
/// that stops abruptly forgets at most this much of when it saw them.
const SAVE_EVERY: f64 = 60.0;

impl Peers {
    /// Peers for the machine `machine`, which shares nothing until it is given a
    /// board with [`Peers::share_on`].
    pub fn new(threshold: f64, machine: impl Into<String>) -> Self {
        Self {
            machine: machine.into(),
            board: Mutex::new(None),
            log: Mutex::new(PresenceLog::new(threshold)),
            user_id: Mutex::new(None),
            log_file: Mutex::new(None),
            log_owner: Mutex::new(None),
            posted_at: Mutex::new(None),
            posting: AtomicBool::new(false),
            write_turn: tokio::sync::Mutex::new(()),
        }
    }

    pub fn machine(&self) -> &str {
        &self.machine
    }

    /// Shares presence on `board` from now on.
    pub fn share_on(&self, board: Arc<dyn Noticeboard>) {
        *lock(&self.board) = Some(board);
    }

    fn board(&self) -> Option<Arc<dyn Noticeboard>> {
        lock(&self.board).clone()
    }

    /// The Harvest user this machine works for, or `None` before one is known.
    pub fn set_user(&self, user_id: Option<i64>) {
        *lock(&self.user_id) = user_id;
        let Some(user_id) = user_id else {
            return;
        };
        let previous = lock(&self.log_owner).replace(user_id);
        match previous {
            Some(previous) if previous == user_id => {}
            Some(_) => {
                // Someone else's presence is no evidence about this user.
                lock(&self.log).intervals.clear();
                self.save_now();
            }
            // Evidence gathered before anyone signed in is of whoever is here now.
            None => self.save_now(),
        }
    }

    /// Keeps the log in `path` from now on, starting from what is there.
    ///
    /// A log kept only in memory was wiped by every relaunch or reboot, and the
    /// next write then told the other machines nobody had been here. The
    /// relaunch itself counts as presence, but only from that moment.
    pub fn keep_log_in(&self, path: PathBuf, now: Timestamp) {
        if let Some(saved) = crate::persistence::read_json::<SavedLog>(&path) {
            let current = *lock(&self.user_id);
            // Restored only for the user it records; a log for someone else is
            // left behind, and overwritten at the next save.
            if current.is_none() || current == saved.user_id {
                *lock(&self.log_owner) = saved.user_id;
                let mut log = lock(&self.log);
                // Anything noted before this was called joins in.
                let mut intervals = saved.intervals;
                intervals.append(&mut log.intervals);
                log.restore(intervals, now);
            }
        }
        *lock(&self.log_file) = Some((path, None));
        self.save(now, true);
    }

    pub fn note_presence(&self, when: Timestamp) {
        let opened = lock(&self.log).note(when);
        self.save(when, opened);
    }

    fn save_now(&self) {
        let last = lock(&self.log_file).as_ref().and_then(|(_, last)| *last);
        if let Some(last) = last {
            self.save(last, true);
        }
    }

    /// Writes the log if it has a file, and either `now_please` or a minute has
    /// passed since the last write.
    fn save(&self, now: Timestamp, now_please: bool) {
        let mut file = lock(&self.log_file);
        let Some((path, last)) = file.as_mut() else {
            return;
        };
        let due = now_please || last.is_none_or(|last| seconds_between(last, now) >= SAVE_EVERY);
        if !due {
            return;
        }
        let saved = SavedLog {
            user_id: *lock(&self.log_owner),
            intervals: lock(&self.log).intervals.clone(),
        };
        match crate::persistence::write_json(path, &saved) {
            Ok(()) => *last = Some(now),
            Err(error) => log::error!("could not save when the user was seen: {error}"),
        }
    }

    /// This machine's stretches that reach `since` or later, as it would share
    /// them. A log not yet known to be the signed-in user's says nothing about
    /// them, so it shares nothing.
    pub fn own_since(&self, since: Timestamp) -> Vec<Interval> {
        let user = *lock(&self.user_id);
        if user.is_none() || *lock(&self.log_owner) != user {
            return Vec::new();
        }
        lock(&self.log).since(since)
    }

    /// Records that the user was here through `absence`, because they said so:
    /// they answered Keep. Written onto the running entry at once, so another
    /// machine asking about the same stretch stops asking when it next looks.
    pub async fn vouch(&self, absence: Absence, now: Timestamp) {
        lock(&self.log).add(Interval {
            start: absence.began,
            end: absence.ended,
        });
        self.save_now();
        self.post(now).await;
    }

    // MARK: Reading

    /// When the other machines saw the user since `since`, as written on the
    /// running entry. Empty with no board, no running timer, or a reference that
    /// is not Sheaves'.
    pub async fn seen_since(&self, since: Timestamp) -> Vec<Interval> {
        let Some(board) = self.board() else {
            return Vec::new();
        };
        let Ok(read) = tokio::time::timeout(READ_WITHIN, board.running()).await else {
            log::info!("Harvest did not answer within five seconds; deciding alone");
            return Vec::new();
        };
        match read {
            Ok(Some(posted)) => match Board::of(posted.reference.as_ref()) {
                Some(read) => read
                    .others(&self.machine)
                    .into_iter()
                    .filter(|stretch| stretch.end >= since)
                    .collect(),
                None => {
                    log::info!("the running entry links elsewhere, so nothing is shared on it");
                    Vec::new()
                }
            },
            Ok(None) => Vec::new(),
            Err(error) => {
                log::info!("deciding without the other machines: {error}");
                Vec::new()
            }
        }
    }

    /// The parts of `absence` that no other machine saw the user in. With no
    /// board, or nothing on it, that is the whole absence.
    pub async fn confirm(&self, absence: Absence, threshold: f64) -> Vec<Absence> {
        if self.board().is_none() {
            return vec![absence];
        }
        let seen = self.seen_since(absence.began).await;
        unwitnessed(absence, &seen, threshold)
    }

    // MARK: Writing

    /// Whether this machine's part is due to be written at `now`: a board to write
    /// on, the user seen since the last write, and a minute gone since it.
    fn post_due(&self, now: Timestamp) -> bool {
        let Some(board) = self.board() else {
            return false;
        };
        if !board.may_post() {
            return false;
        }
        let Some(newest) = lock(&self.log).intervals.last().map(|last| last.end) else {
            return false;
        };
        match *lock(&self.posted_at) {
            None => true,
            Some(posted) => newest > posted && seconds_between(posted, now) >= POST_EVERY,
        }
    }

    /// Writes this machine's part onto the running entry, keeping the others':
    /// its stretches since the timer started, read first so as not to undo what
    /// another machine wrote since. Does nothing when no timer runs or the entry
    /// links elsewhere.
    pub async fn post(&self, now: Timestamp) {
        let _turn = self.write_turn.lock().await;
        let Some(board) = self.board() else {
            return;
        };
        // Not yet known to be the signed-in user's, the log has nothing to say,
        // and writing that would erase this machine's part.
        let user = *lock(&self.user_id);
        if user.is_none() || *lock(&self.log_owner) != user {
            return;
        }
        *lock(&self.posted_at) = Some(now);
        let posted = match board.running().await {
            Ok(Some(posted)) => posted,
            Ok(None) => return,
            Err(error) => {
                log::info!("could not read the running entry to share presence: {error}");
                return;
            }
        };
        let Some(mut shared) = Board::of(posted.reference.as_ref()) else {
            return;
        };
        let started = posted.timer_started_at;
        let own: Vec<Interval> = self
            .own_since(started)
            .into_iter()
            .map(|stretch| Interval {
                start: stretch.start.max(started),
                end: stretch.end,
            })
            .collect();
        if own.is_empty() && !shared.parts.contains_key(&self.machine) {
            return;
        }
        shared.parts.insert(self.machine.clone(), own);
        let reference = shared.reference();
        if posted.reference.as_ref() == Some(&reference) {
            return;
        }
        if let Err(error) = board.post(posted.entry_id, &reference).await {
            log::info!("could not share presence on the running entry: {error}");
        }
    }

    /// [`Peers::post`], started in the background when due, and never twice at
    /// once: a poll must not wait on Harvest.
    pub fn post_soon(self: &Arc<Self>, now: Timestamp) {
        if !self.post_due(now) || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        if self.posting.swap(true, Ordering::SeqCst) {
            return;
        }
        let peers = self.clone();
        tokio::spawn(async move {
            peers.post(now).await;
            peers.posting.store(false, Ordering::SeqCst);
        });
    }
}

// MARK: Presence with peers

/// A [`PresenceMonitor`] that logs what it sees and shares it, and reads what the
/// other machines saw before reporting an absence. Each shell hands it a reading
/// every poll.
pub struct Presence {
    monitor: Mutex<PresenceMonitor>,
    threshold: f64,
    peers: Arc<Peers>,
    /// Polls take turns, so the absences they report come out in order even
    /// while one of them waits on Harvest.
    turn: tokio::sync::Mutex<()>,
}

impl Presence {
    pub fn new(threshold: f64, now: Timestamp, peers: Arc<Peers>) -> Self {
        // The detector counts the launch as presence, so the log does too.
        peers.note_presence(now);
        Self {
            monitor: Mutex::new(PresenceMonitor::new(threshold, now)),
            threshold,
            peers,
            turn: tokio::sync::Mutex::new(()),
        }
    }

    /// Folds one reading taken at `now`, and returns the absences it ended that no
    /// other machine saw the user during.
    pub async fn poll(&self, mut reading: PresenceReading, now: Timestamp) -> Vec<Absence> {
        let _turn = self.turn.lock().await;
        // A sensor's number is not to be trusted with arithmetic on instants: an
        // infinite or absurd one would panic, and with the lock held every later
        // poll would too. A reading that makes no sense says nothing about absence,
        // so it counts as presence now.
        const LONGEST_IDLE: f64 = 10.0 * 365.0 * 86_400.0;
        let idle = reading.seconds_since_input;
        reading.seconds_since_input = if idle.is_finite() {
            idle.clamp(0.0, LONGEST_IDLE)
        } else {
            0.0
        };
        let (candidates, evidence) = {
            let mut monitor = lock(&self.monitor);
            let candidates = monitor.poll(reading, now);
            (candidates, monitor.take_evidence())
        };
        // Outside the monitor's lock: noting can write the log to disk.
        for when in evidence {
            self.peers.note_presence(when);
        }
        self.peers.post_soon(now);
        let mut absences = Vec::new();
        for candidate in candidates {
            absences.extend(self.peers.confirm(candidate, self.threshold).await);
        }
        absences
    }
}

/// Locks `mutex`, carrying on past a panic elsewhere: everything held under these
/// locks is replaced whole, so a poisoned one still holds a usable value.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests;
