//! Sharing presence between one person's machines, over the tailnet.
//!
//! Start a timer on the Mac at nine, work at Hostess from ten to twelve, come back
//! to the Mac: alone, the Mac sees two hours with nobody at it and offers to trim
//! them. It lacks only Hostess's evidence. So each machine keeps a log of when it
//! saw the user, answers its peers' questions about that log, and asks them before
//! it reports an absence. Each machine also tells the others when it has changed a
//! timer, so they look at Harvest at once instead of at their next probe.
//!
//! The tailnet is what authenticates and encrypts this: every device on it is the
//! user's, and WireGuard carries each request. See `docs/gnome-port.md` for why the
//! local network is not used.

pub mod server;

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::absence::{Absence, PresenceMonitor, PresenceReading};
use crate::models::{offset_by, seconds_between};

/// The port every instance listens on and asks on.
pub const PEER_PORT: u16 = 47147;

/// How long a peer has to answer before this machine decides alone.
pub const ANSWER_WITHIN: Duration = Duration::from_secs(2);

/// How far back the presence log reaches. A day covers any absence worth asking
/// about; a timer left running longer than that is a different problem.
const LOG_SPAN: f64 = 24.0 * 3600.0;

/// A stretch during which a machine had evidence of the user, with no gap in it as
/// long as the absence threshold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

// MARK: Messages

/// A peer's answer to "when did you see the user since this moment?".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PresenceAnswer {
    /// Whose presence this is. An answer about anyone else is ignored.
    pub user_id: i64,
    pub intervals: Vec<Interval>,
}

/// "I just changed these entries in Harvest."
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Touched {
    pub user_id: i64,
    pub entry_ids: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum PeerError {
    #[error("{peer} is not on the tailnet: its name resolves to {addresses}")]
    NotOnTailnet { peer: String, addresses: String },
    #[error("could not reach {peer}: {reason}")]
    Unreachable { peer: String, reason: String },
    #[error("{peer} refused: {status}")]
    Refused { peer: String, status: u16 },
    #[error("{peer} answered with something unreadable: {detail}")]
    Unreadable { peer: String, detail: String },
}

/// How this machine reaches its peers. HTTP in the apps; tests join two instances
/// directly.
#[async_trait]
pub trait PeerTransport: Send + Sync {
    async fn presence(
        &self,
        peer: &str,
        user_id: i64,
        since: Timestamp,
    ) -> Result<PresenceAnswer, PeerError>;

    async fn touched(&self, peer: &str, touched: &Touched) -> Result<(), PeerError>;
}

// MARK: The tailnet

/// Whether `address` is one Tailscale hands out: `100.64.0.0/10` or
/// `fd7a:115c:a1e0::/48`.
pub fn is_tailnet(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            a == 100 && (64..128).contains(&b)
        }
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_tailnet(IpAddr::V4(v4)),
            None => {
                let s = v6.segments();
                s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
            }
        },
    }
}

/// This machine's own tailnet addresses, empty when Tailscale is not running.
pub fn tailnet_addresses() -> Vec<IpAddr> {
    let mut addresses: Vec<IpAddr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|interface| interface.ip())
        .filter(|address| is_tailnet(*address))
        .collect();
    addresses.sort();
    addresses.dedup();
    addresses
}

/// Peers over plain HTTP, reached only at tailnet addresses.
pub struct HttpPeerTransport {
    client: reqwest::Client,
    port: u16,
}

impl HttpPeerTransport {
    pub fn new(port: u16) -> Self {
        Self {
            client: reqwest::Client::builder()
                // A proxy would carry the request off the tailnet.
                .no_proxy()
                // Peers are asked a few times a day. A connection kept open between
                // times only waits for the peer to close it, which the peer logs as
                // a client that never sent its next request.
                .pool_max_idle_per_host(0)
                .timeout(ANSWER_WITHIN)
                .build()
                .expect("an HTTP client with no TLS settings builds"),
            port,
        }
    }

    /// Where `peer` is on the tailnet. A name that resolves anywhere else is not
    /// asked at all: the answer would cross a network nothing authenticates.
    async fn address(&self, peer: &str) -> Result<SocketAddr, PeerError> {
        let resolved: Vec<SocketAddr> = tokio::net::lookup_host((peer, self.port))
            .await
            .map_err(|error| PeerError::Unreachable {
                peer: peer.to_string(),
                reason: error.to_string(),
            })?
            .collect();
        resolved
            .iter()
            .find(|address| is_tailnet(address.ip()))
            .copied()
            .ok_or_else(|| PeerError::NotOnTailnet {
                peer: peer.to_string(),
                addresses: resolved
                    .iter()
                    .map(|address| address.ip().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            })
    }

    async fn read(
        &self,
        peer: &str,
        request: reqwest::RequestBuilder,
    ) -> Result<Vec<u8>, PeerError> {
        let response = request
            .send()
            .await
            .map_err(|error| PeerError::Unreachable {
                peer: peer.to_string(),
                reason: error.to_string(),
            })?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(PeerError::Refused {
                peer: peer.to_string(),
                status,
            });
        }
        let body = response
            .bytes()
            .await
            .map_err(|error| PeerError::Unreachable {
                peer: peer.to_string(),
                reason: error.to_string(),
            })?;
        Ok(body.to_vec())
    }
}

#[async_trait]
impl PeerTransport for HttpPeerTransport {
    async fn presence(
        &self,
        peer: &str,
        user_id: i64,
        since: Timestamp,
    ) -> Result<PresenceAnswer, PeerError> {
        let address = self.address(peer).await?;
        let mut url = url::Url::parse(&format!("http://{address}/presence"))
            .expect("a socket address makes a URL");
        url.query_pairs_mut()
            .append_pair("since", &since.to_string())
            .append_pair("user_id", &user_id.to_string());
        let body = self.read(peer, self.client.get(url)).await?;
        serde_json::from_slice(&body).map_err(|error| PeerError::Unreadable {
            peer: peer.to_string(),
            detail: error.to_string(),
        })
    }

    async fn touched(&self, peer: &str, touched: &Touched) -> Result<(), PeerError> {
        let address = self.address(peer).await?;
        let request = self
            .client
            .post(format!("http://{address}/touched"))
            .header("content-type", "application/json")
            .body(serde_json::to_vec(touched).expect("a notice encodes"));
        self.read(peer, request).await.map(|_| ())
    }
}

// MARK: Peers

/// Why this machine will not answer a peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// It does not know whose machine it is yet, so it cannot check.
    NotSignedIn,
    /// The question is about someone else.
    OtherUser,
}

type TouchedHandler = Arc<dyn Fn(Vec<i64>) + Send + Sync>;

/// This machine's side of the conversation: the log it answers from, the peers
/// it asks, and what it does when told a timer changed.
pub struct Peers {
    names: Mutex<Vec<String>>,
    transport: Arc<dyn PeerTransport>,
    log: Mutex<PresenceLog>,
    user_id: Mutex<Option<i64>>,
    on_touched: Mutex<Option<TouchedHandler>>,
    /// Where the log is kept between launches, and when it was last written.
    log_file: Mutex<Option<(PathBuf, Option<Timestamp>)>>,
    /// Whose presence the log records. Evidence is of whoever is at the machine,
    /// but the log is answered for one Harvest user, so a different user signing in
    /// starts it afresh.
    log_owner: Mutex<Option<i64>>,
}

/// The log as kept on disk, with whose it is.
#[derive(Serialize, Deserialize)]
struct SavedLog {
    user_id: Option<i64>,
    intervals: Vec<Interval>,
}

/// How often the log is written while the user is present: a machine that stops
/// abruptly forgets at most this much of when it saw them.
const SAVE_EVERY: f64 = 60.0;

impl Peers {
    pub fn new(threshold: f64, transport: Arc<dyn PeerTransport>) -> Self {
        Self {
            names: Mutex::new(Vec::new()),
            transport,
            log: Mutex::new(PresenceLog::new(threshold)),
            user_id: Mutex::new(None),
            on_touched: Mutex::new(None),
            log_file: Mutex::new(None),
            log_owner: Mutex::new(None),
        }
    }

    /// The machines to ask, by their names on the tailnet, such as `hostess`.
    pub fn set_names(&self, names: Vec<String>) {
        let mut cleaned: Vec<String> = Vec::new();
        for name in names {
            let name = name.trim().to_string();
            if !name.is_empty() && !cleaned.contains(&name) {
                cleaned.push(name);
            }
        }
        *lock(&self.names) = cleaned;
    }

    pub fn names(&self) -> Vec<String> {
        lock(&self.names).clone()
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

    /// What to do when a peer says it changed entries: probe Harvest.
    pub fn on_touched(&self, handler: impl Fn(Vec<i64>) + Send + Sync + 'static) {
        *lock(&self.on_touched) = Some(Arc::new(handler));
    }

    /// Keeps the log in `path` from now on, starting from what is there.
    ///
    /// A log kept only in memory was wiped by every relaunch or reboot, and a peer
    /// asking about the hour before one was then told nobody had been here. The
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

    // MARK: Asking

    /// When the peers saw the user since `since`, from every peer that answered in
    /// time about this user. Empty when there are no peers or none answered.
    pub async fn seen_since(&self, since: Timestamp) -> Vec<Interval> {
        let names = self.names();
        let Some(user_id) = *lock(&self.user_id) else {
            if !names.is_empty() {
                log::info!("asked no peers: not signed in, so their answers could not be checked");
            }
            return Vec::new();
        };
        let mut asking = tokio::task::JoinSet::new();
        for name in names {
            let transport = self.transport.clone();
            asking.spawn(async move {
                let answer =
                    tokio::time::timeout(ANSWER_WITHIN, transport.presence(&name, user_id, since))
                        .await;
                (name, answer)
            });
        }
        let mut seen = Vec::new();
        while let Some(joined) = asking.join_next().await {
            let Ok((name, answer)) = joined else { continue };
            match answer {
                Err(_) => {
                    log::info!("{name} did not answer within two seconds; deciding without it")
                }
                Ok(Err(error)) => log::info!("deciding without {name}: {error}"),
                Ok(Ok(answer)) if answer.user_id != user_id => {
                    log::warn!("ignored {name}: it reported presence for another Harvest user")
                }
                Ok(Ok(answer)) => seen.extend(answer.intervals),
            }
        }
        seen
    }

    /// The parts of `absence` that no peer saw the user in. With no peers, or none
    /// answering, that is the whole absence, as it was before peers existed.
    pub async fn confirm(&self, absence: Absence, threshold: f64) -> Vec<Absence> {
        if self.names().is_empty() {
            return vec![absence];
        }
        let seen = self.seen_since(absence.began).await;
        unwitnessed(absence, &seen, threshold)
    }

    /// Tells every peer that this machine changed `entry_ids` in Harvest.
    pub async fn announce(&self, entry_ids: Vec<i64>) {
        let Some(user_id) = *lock(&self.user_id) else {
            return;
        };
        let touched = Touched { user_id, entry_ids };
        let mut telling = tokio::task::JoinSet::new();
        for name in self.names() {
            let transport = self.transport.clone();
            let touched = touched.clone();
            telling.spawn(async move {
                let told =
                    tokio::time::timeout(ANSWER_WITHIN, transport.touched(&name, &touched)).await;
                (name, told)
            });
        }
        while let Some(joined) = telling.join_next().await {
            match joined {
                Ok((name, Ok(Err(error)))) => log::info!("could not tell {name}: {error}"),
                Ok((name, Err(_))) => log::info!("could not tell {name}: no answer in time"),
                _ => {}
            }
        }
    }

    // MARK: Answering

    /// This machine's log since `since`, for the peer asking on behalf of
    /// `user_id`.
    pub fn answer_presence(
        &self,
        user_id: i64,
        since: Timestamp,
    ) -> Result<PresenceAnswer, Refusal> {
        let own = self.check_user(user_id)?;
        // A log not yet known to be this user's says nothing about them.
        let intervals = if *lock(&self.log_owner) == Some(own) {
            lock(&self.log).since(since)
        } else {
            Vec::new()
        };
        Ok(PresenceAnswer {
            user_id: own,
            intervals,
        })
    }

    /// A peer changed entries in Harvest.
    pub fn receive_touched(&self, touched: Touched) -> Result<(), Refusal> {
        self.check_user(touched.user_id)?;
        log::info!(
            "a peer changed entries {:?}; probing Harvest",
            touched.entry_ids
        );
        let handler = lock(&self.on_touched).clone();
        if let Some(handler) = handler {
            handler(touched.entry_ids);
        }
        Ok(())
    }

    fn check_user(&self, user_id: i64) -> Result<i64, Refusal> {
        match *lock(&self.user_id) {
            None => Err(Refusal::NotSignedIn),
            Some(own) if own != user_id => Err(Refusal::OtherUser),
            Some(own) => Ok(own),
        }
    }
}

// MARK: Presence with peers

/// A [`PresenceMonitor`] that logs what it sees for its peers, and asks them
/// before reporting an absence. Each shell hands it a reading every poll.
pub struct Presence {
    monitor: Mutex<PresenceMonitor>,
    threshold: f64,
    peers: Arc<Peers>,
    /// Polls take turns, so the absences they report come out in order even
    /// while one of them waits on a peer.
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
    /// peer saw the user during.
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
