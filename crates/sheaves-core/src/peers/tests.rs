use super::*;

const THRESHOLD: f64 = 15.0 * 60.0;
const USER: i64 = 1782959;

/// Minutes after eight in the morning.
fn at(minutes: f64) -> Timestamp {
    offset_by("2026-08-29T08:00:00Z".parse().unwrap(), minutes * 60.0)
}

fn span(start: f64, end: f64) -> Interval {
    Interval {
        start: at(start),
        end: at(end),
    }
}

fn absence(began: f64, ended: f64) -> Absence {
    Absence {
        began: at(began),
        ended: at(ended),
    }
}

mod presence_log {
    use super::*;

    #[test]
    fn joins_evidence_closer_than_the_threshold() {
        let mut log = PresenceLog::new(THRESHOLD);
        for minute in [0.0, 5.0, 19.0, 33.0] {
            log.note(at(minute));
        }
        assert_eq!(log.since(at(0.0)), [span(0.0, 33.0)]);
    }

    /// A gap as long as the threshold is an absence, and the log keeps it as one.
    #[test]
    fn keeps_a_gap_as_long_as_the_threshold() {
        let mut log = PresenceLog::new(THRESHOLD);
        log.note(at(0.0));
        log.note(at(15.0));
        assert_eq!(log.since(at(0.0)), [span(0.0, 0.0), span(15.0, 15.0)]);
    }

    /// A call is dated to the poll before, so evidence can arrive older than what
    /// the log already holds.
    #[test]
    fn places_late_evidence_where_it_belongs() {
        let mut log = PresenceLog::new(THRESHOLD);
        log.note(at(0.0));
        log.note(at(40.0));
        log.note(at(28.0));
        assert_eq!(log.since(at(0.0)), [span(0.0, 0.0), span(28.0, 40.0)]);
        // And evidence between two intervals joins them both.
        log.note(at(14.0));
        assert_eq!(log.since(at(0.0)), [span(0.0, 40.0)]);
    }

    #[test]
    fn answers_from_the_moment_asked_about() {
        let mut log = PresenceLog::new(THRESHOLD);
        for minute in [0.0, 10.0, 60.0, 70.0] {
            log.note(at(minute));
        }
        // An interval that reaches past `since` is answered whole: the asker needs
        // to know the user was seen at `since`.
        assert_eq!(log.since(at(5.0)), [span(0.0, 10.0), span(60.0, 70.0)]);
        assert_eq!(log.since(at(30.0)), [span(60.0, 70.0)]);
    }

    #[test]
    fn forgets_what_is_more_than_a_day_old() {
        let mut log = PresenceLog::new(THRESHOLD);
        log.note(at(0.0));
        log.note(at(25.0 * 60.0));
        assert_eq!(log.since(at(0.0)), [span(25.0 * 60.0, 25.0 * 60.0)]);
    }
}

/// The log outlives the app. Kept only in memory, it was wiped by each relaunch,
/// and a peer asking about the hour before one was told nobody had been here: the
/// Mac then asked about time spent working at Hostess.
mod kept_on_disk {
    use super::*;

    fn peers(path: &std::path::Path, now: Timestamp) -> Peers {
        let peers = Peers::new(THRESHOLD, "k3x9");
        peers.set_user(Some(USER));
        peers.keep_log_in(path.to_path_buf(), now);
        peers
    }

    #[test]
    fn survives_a_relaunch() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("presence.json");
        let before = peers(&path, at(0.0));
        for minute in (0..=60).step_by(5) {
            before.note_presence(at(f64::from(minute)));
        }
        drop(before);

        let after = peers(&path, at(90.0));
        after.note_presence(at(90.0));
        assert_eq!(
            after.own_since(at(0.0)),
            [span(0.0, 60.0), span(90.0, 90.0)]
        );
    }

    /// Written at most once a minute while evidence streams in, but at once when a
    /// new interval opens, since that is what a peer's question turns on.
    #[test]
    fn writes_a_new_interval_at_once_and_the_rest_each_minute() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("presence.json");
        let saved = || {
            crate::persistence::read_json::<serde_json::Value>(&path).unwrap()["intervals"].clone()
        };
        let saved = || serde_json::from_value::<Vec<Interval>>(saved()).unwrap();
        let peers = peers(&path, at(0.0));

        peers.note_presence(at(0.0));
        assert_eq!(saved(), [span(0.0, 0.0)]);
        peers.note_presence(at(0.5));
        assert_eq!(saved(), [span(0.0, 0.0)], "not yet a minute");
        peers.note_presence(at(1.0));
        assert_eq!(saved(), [span(0.0, 1.0)]);
        peers.note_presence(at(30.0));
        assert_eq!(
            saved(),
            [span(0.0, 1.0), span(30.0, 30.0)],
            "a new interval"
        );
    }

    /// Presence is answered for one Harvest user, so a log kept for someone else
    /// is neither restored nor served.
    #[test]
    fn another_users_log_is_not_restored() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("presence.json");
        let before = peers(&path, at(0.0));
        before.note_presence(at(0.0));
        before.note_presence(at(30.0));
        drop(before);

        let other = Peers::new(THRESHOLD, "k3x9");
        other.set_user(Some(USER + 1));
        other.keep_log_in(path.clone(), at(40.0));
        assert_eq!(other.own_since(at(0.0)), []);
    }

    #[test]
    fn signing_in_as_someone_else_starts_afresh() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("presence.json");
        let peers = peers(&path, at(0.0));
        peers.note_presence(at(0.0));
        peers.set_user(Some(USER + 1));
        assert_eq!(peers.own_since(at(0.0)), []);
        peers.note_presence(at(5.0));
        assert_eq!(peers.own_since(at(0.0)), [span(5.0, 5.0)]);
    }

    /// Evidence noted before anyone signed in, such as the launch, is kept for
    /// whoever then does.
    #[test]
    fn evidence_from_before_signing_in_is_kept() {
        let peers = Peers::new(THRESHOLD, "k3x9");
        peers.note_presence(at(0.0));
        peers.set_user(Some(USER));
        assert_eq!(peers.own_since(at(0.0)), [span(0.0, 0.0)]);
    }

    /// A new interval is reported as one even when the same note drops a day-old
    /// interval, which leaves the count unchanged.
    #[test]
    fn a_new_interval_counts_as_new_while_an_old_one_goes() {
        let mut log = PresenceLog::new(THRESHOLD);
        assert!(log.note(at(0.0)));
        assert!(!log.note(at(5.0)));
        assert!(log.note(at(25.0 * 60.0)));
        assert_eq!(log.intervals(), [span(25.0 * 60.0, 25.0 * 60.0)]);
    }

    /// A long stretch noted before the log was loaded joins what the file held,
    /// whole, rather than its two ends standing apart with an absence between.
    #[test]
    fn evidence_noted_before_loading_joins_the_file() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("presence.json");
        let first = peers(&path, at(0.0));
        first.note_presence(at(0.0));
        first.note_presence(at(10.0));
        drop(first);

        let peers = Peers::new(THRESHOLD, "k3x9");
        peers.set_user(Some(USER));
        for minute in [12.0, 20.0, 30.0] {
            peers.note_presence(at(minute));
        }
        peers.keep_log_in(path, at(30.0));
        assert_eq!(peers.own_since(at(0.0)), [span(0.0, 30.0)]);
    }

    /// The order the apps use: the log is loaded before anyone is signed in, and
    /// the tracker names the user afterwards.
    #[test]
    fn loaded_before_signing_in_as_the_apps_do() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("presence.json");
        let first = peers(&path, at(0.0));
        first.note_presence(at(0.0));
        first.note_presence(at(10.0));
        drop(first);

        let same = Peers::new(THRESHOLD, "k3x9");
        same.keep_log_in(path.clone(), at(20.0));
        same.set_user(Some(USER));
        assert_eq!(same.own_since(at(0.0)), [span(0.0, 10.0)]);
        drop(same);

        let other = Peers::new(THRESHOLD, "k3x9");
        other.keep_log_in(path, at(20.0));
        other.set_user(Some(USER + 1));
        assert_eq!(other.own_since(at(0.0)), []);
    }

    /// A stretch from a clock that ran ahead is not restored: it would never
    /// expire, and would vouch for the user over any absence.
    #[test]
    fn a_stretch_from_the_future_is_not_restored() {
        let mut log = PresenceLog::new(THRESHOLD);
        log.restore(
            vec![span(0.0, 10.0), span(5.0, 40.0), span(500.0, 600.0)],
            at(20.0),
        );
        assert_eq!(log.intervals(), [span(0.0, 20.0)]);
    }

    #[test]
    fn reads_back_only_the_last_day_and_only_sound_intervals() {
        let mut log = PresenceLog::new(THRESHOLD);
        let backwards = Interval {
            start: at(50.0),
            end: at(40.0),
        };
        log.restore(
            vec![
                span(60.0, 70.0),
                span(0.0, 10.0),
                backwards,
                span(-2000.0, -1990.0),
            ],
            at(80.0),
        );
        assert_eq!(log.intervals(), [span(0.0, 10.0), span(60.0, 70.0)]);
        // And joined where a damaged file left them closer than the threshold.
        log.restore(vec![span(0.0, 10.0), span(14.0, 20.0)], at(30.0));
        assert_eq!(log.intervals(), [span(0.0, 20.0)]);
    }
}

mod merge {
    use super::*;

    /// The case peers exist for. The timer started on the Mac at nine; the user
    /// worked at Hostess from ten to twelve and came back to the Mac at five past.
    #[test]
    fn work_at_the_other_machine_is_not_an_absence() {
        assert_eq!(
            unwitnessed(absence(120.0, 245.0), &[span(120.0, 240.0)], THRESHOLD),
            []
        );
    }

    #[test]
    fn nobody_seen_anywhere_is_the_whole_absence() {
        assert_eq!(
            unwitnessed(absence(120.0, 245.0), &[], THRESHOLD),
            [absence(120.0, 245.0)]
        );
    }

    /// Seen at the other machine for half an hour in the middle of the morning:
    /// the stretches either side are each long enough to ask about.
    #[test]
    fn splits_around_what_a_peer_saw() {
        assert_eq!(
            unwitnessed(absence(120.0, 245.0), &[span(150.0, 180.0)], THRESHOLD),
            [absence(120.0, 150.0), absence(180.0, 245.0)]
        );
    }

    /// Two peers' answers arrive in any order, and may overlap.
    #[test]
    fn order_and_overlap_do_not_matter() {
        let seen = [span(200.0, 230.0), span(130.0, 190.0), span(150.0, 210.0)];
        assert_eq!(
            unwitnessed(absence(120.0, 245.0), &seen, THRESHOLD),
            [absence(230.0, 245.0)]
        );
        let mut reversed = seen;
        reversed.reverse();
        assert_eq!(
            unwitnessed(absence(120.0, 245.0), &reversed, THRESHOLD),
            [absence(230.0, 245.0)]
        );
    }

    #[test]
    fn ignores_what_lies_outside_the_absence() {
        assert_eq!(
            unwitnessed(
                absence(120.0, 245.0),
                &[span(0.0, 100.0), span(300.0, 400.0)],
                THRESHOLD
            ),
            [absence(120.0, 245.0)]
        );
    }

    #[test]
    fn a_remainder_under_the_threshold_is_not_asked_about() {
        assert_eq!(
            unwitnessed(absence(120.0, 245.0), &[span(115.0, 231.0)], THRESHOLD),
            []
        );
    }
}

mod sharing {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    /// A running entry in memory, counting the writes it is sent.
    struct Entry {
        reference: Mutex<Option<ExternalReference>>,
        posts: AtomicUsize,
        low_power: AtomicBool,
    }

    impl Entry {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                reference: Mutex::new(None),
                posts: AtomicUsize::new(0),
                low_power: AtomicBool::new(false),
            })
        }
    }

    #[async_trait]
    impl Noticeboard for Entry {
        async fn running(&self) -> Result<Option<Posted>, String> {
            Ok(Some(Posted {
                entry_id: 1,
                timer_started_at: at(0.0),
                reference: self.reference.lock().unwrap().clone(),
            }))
        }

        async fn post(&self, _: i64, reference: &ExternalReference) -> Result<(), String> {
            self.posts.fetch_add(1, Ordering::SeqCst);
            *self.reference.lock().unwrap() = Some(reference.clone());
            Ok(())
        }

        fn may_post(&self) -> bool {
            !self.low_power.load(Ordering::SeqCst)
        }
    }

    fn sharing(entry: &Arc<Entry>, machine: &str) -> Arc<Peers> {
        let peers = Arc::new(Peers::new(THRESHOLD, machine));
        peers.set_user(Some(USER));
        peers.share_on(entry.clone());
        peers
    }

    #[tokio::test]
    async fn reads_what_the_other_machine_wrote_and_not_its_own() {
        let entry = Entry::new();
        let mac = sharing(&entry, "mac");
        let hostess = sharing(&entry, "host");
        mac.note_presence(at(10.0));
        mac.post(at(10.0)).await;
        for minute in (60..=120).step_by(10) {
            hostess.note_presence(at(minute as f64));
        }
        hostess.post(at(120.0)).await;

        assert_eq!(mac.seen_since(at(0.0)).await, [span(60.0, 120.0)]);
        assert_eq!(hostess.seen_since(at(0.0)).await, [span(10.0, 10.0)]);
    }

    #[tokio::test]
    async fn an_unchanged_part_is_not_written_again() {
        let entry = Entry::new();
        let mac = sharing(&entry, "mac");
        mac.note_presence(at(10.0));
        mac.post(at(10.0)).await;
        mac.post(at(11.0)).await;
        assert_eq!(entry.posts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn writes_nothing_in_low_power_mode() {
        let entry = Entry::new();
        entry.low_power.store(true, Ordering::SeqCst);
        let mac = sharing(&entry, "mac");
        mac.note_presence(at(10.0));
        assert!(!mac.post_due(at(10.0)));
        // Reading for a question still happens.
        entry.low_power.store(false, Ordering::SeqCst);
        assert!(mac.post_due(at(10.0)));
    }

    /// Relaunched while the timer runs, the entry already holds this machine's
    /// part; a write before the user is known would erase it.
    #[tokio::test]
    async fn shares_nothing_before_it_knows_whose_machine_it_is() {
        let entry = Entry::new();
        let earlier = Board {
            parts: [("mac".to_string(), vec![span(1.0, 5.0)])].into(),
        };
        *entry.reference.lock().unwrap() = Some(earlier.reference());
        let mac = Arc::new(Peers::new(THRESHOLD, "mac"));
        mac.share_on(entry.clone());
        mac.note_presence(at(10.0));
        mac.post(at(10.0)).await;
        assert_eq!(entry.posts.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn is_due_a_minute_after_the_last_write_and_only_with_news() {
        let entry = Entry::new();
        let mac = sharing(&entry, "mac");
        mac.note_presence(at(10.0));
        assert!(mac.post_due(at(10.0)));
        *lock(&mac.posted_at) = Some(at(10.0));
        mac.note_presence(at(10.5));
        assert!(!mac.post_due(at(10.5)));
        assert!(mac.post_due(at(11.0)));
        // Nobody seen since the last write: nothing to say.
        *lock(&mac.posted_at) = Some(at(11.0));
        assert!(!mac.post_due(at(20.0)));
    }

    #[test]
    fn keeps_its_machine_id() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("machine.json");
        let first = machine_id_in(&path);
        assert!(board::is_machine_id(&first), "{first:?}");
        assert_eq!(machine_id_in(&path), first);
    }
}
