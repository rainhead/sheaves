use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use hyper::{Method, StatusCode};

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
        let peers = Peers::new(THRESHOLD, Arc::new(Silent));
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
            after.answer_presence(USER, at(0.0)).unwrap().intervals,
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
            crate::persistence::read_json::<serde_json::Value>(&path).unwrap()["intervals"]
                .clone()
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
        assert_eq!(saved(), [span(0.0, 1.0), span(30.0, 30.0)], "a new interval");
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

        let other = Peers::new(THRESHOLD, Arc::new(Silent));
        other.set_user(Some(USER + 1));
        other.keep_log_in(path.clone(), at(40.0));
        assert_eq!(other.answer_presence(USER + 1, at(0.0)).unwrap().intervals, []);
    }

    #[test]
    fn signing_in_as_someone_else_starts_afresh() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("presence.json");
        let peers = peers(&path, at(0.0));
        peers.note_presence(at(0.0));
        peers.set_user(Some(USER + 1));
        assert_eq!(peers.answer_presence(USER + 1, at(0.0)).unwrap().intervals, []);
        peers.note_presence(at(5.0));
        assert_eq!(
            peers.answer_presence(USER + 1, at(0.0)).unwrap().intervals,
            [span(5.0, 5.0)]
        );
    }

    /// Evidence noted before anyone signed in, such as the launch, is kept for
    /// whoever then does.
    #[test]
    fn evidence_from_before_signing_in_is_kept() {
        let peers = Peers::new(THRESHOLD, Arc::new(Silent));
        peers.note_presence(at(0.0));
        peers.set_user(Some(USER));
        assert_eq!(peers.answer_presence(USER, at(0.0)).unwrap().intervals, [span(0.0, 0.0)]);
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

    #[test]
    fn reads_back_only_the_last_day_and_only_sound_intervals() {
        let mut log = PresenceLog::new(THRESHOLD);
        let backwards = Interval {
            start: at(50.0),
            end: at(40.0),
        };
        log.restore(
            vec![span(60.0, 70.0), span(0.0, 10.0), backwards, span(-2000.0, -1990.0)],
            at(80.0),
        );
        assert_eq!(log.intervals(), [span(0.0, 10.0), span(60.0, 70.0)]);
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

#[test]
fn knows_the_tailnet_ranges() {
    for inside in [
        "100.64.0.1",
        "100.127.255.254",
        "fd7a:115c:a1e0::1",
        "::ffff:100.100.1.2",
    ] {
        assert!(is_tailnet(inside.parse().unwrap()), "{inside}");
    }
    for outside in [
        "100.63.255.255",
        "100.128.0.0",
        "192.168.1.10",
        "127.0.0.1",
        "fd7a:115c:a1e1::1",
        "::1",
    ] {
        assert!(!is_tailnet(outside.parse().unwrap()), "{outside}");
    }
}

/// Joins `Peers` to one another in memory, by name.
#[derive(Default)]
struct Direct {
    peers: Mutex<HashMap<String, Arc<Peers>>>,
    asked: AtomicUsize,
}

#[async_trait]
impl PeerTransport for Direct {
    async fn presence(
        &self,
        peer: &str,
        user_id: i64,
        since: Timestamp,
    ) -> Result<PresenceAnswer, PeerError> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        let target = self.peers.lock().unwrap().get(peer).cloned();
        let target = target.ok_or_else(|| PeerError::Unreachable {
            peer: peer.to_string(),
            reason: "no such machine".to_string(),
        })?;
        target
            .answer_presence(user_id, since)
            .map_err(|_| PeerError::Refused {
                peer: peer.to_string(),
                status: 403,
            })
    }

    async fn touched(&self, peer: &str, touched: &Touched) -> Result<(), PeerError> {
        let target = self.peers.lock().unwrap().get(peer).cloned();
        if let Some(target) = target {
            let _ = target.receive_touched(touched.clone());
        }
        Ok(())
    }
}

/// Answers every question about a different user than the one asked about.
struct Impostor;

#[async_trait]
impl PeerTransport for Impostor {
    async fn presence(
        &self,
        _: &str,
        user_id: i64,
        since: Timestamp,
    ) -> Result<PresenceAnswer, PeerError> {
        Ok(PresenceAnswer {
            user_id: user_id + 1,
            intervals: vec![Interval {
                start: since,
                end: offset_by(since, 86_400.0),
            }],
        })
    }

    async fn touched(&self, _: &str, _: &Touched) -> Result<(), PeerError> {
        Ok(())
    }
}

/// Never answers.
struct Silent;

#[async_trait]
impl PeerTransport for Silent {
    async fn presence(&self, _: &str, _: i64, _: Timestamp) -> Result<PresenceAnswer, PeerError> {
        std::future::pending().await
    }

    async fn touched(&self, _: &str, _: &Touched) -> Result<(), PeerError> {
        std::future::pending().await
    }
}

fn signed_in(transport: Arc<dyn PeerTransport>, names: &[&str]) -> Arc<Peers> {
    let peers = Arc::new(Peers::new(THRESHOLD, transport));
    peers.set_user(Some(USER));
    peers.set_names(names.iter().map(ToString::to_string).collect());
    peers
}

mod asking {
    use super::*;

    #[tokio::test]
    async fn with_no_peers_the_absence_stands_and_nobody_is_asked() {
        let direct = Arc::new(Direct::default());
        let mac = signed_in(direct.clone(), &[]);
        assert_eq!(
            mac.confirm(absence(120.0, 245.0), THRESHOLD).await,
            [absence(120.0, 245.0)]
        );
        assert_eq!(direct.asked.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_peer_that_saw_the_user_answers_the_question() {
        let direct = Arc::new(Direct::default());
        let mac = signed_in(direct.clone(), &["hostess"]);
        let hostess = signed_in(direct.clone(), &["mac"]);
        direct
            .peers
            .lock()
            .unwrap()
            .insert("hostess".into(), hostess.clone());
        for minute in (120..=240).step_by(5) {
            hostess.note_presence(at(f64::from(minute)));
        }
        assert_eq!(mac.confirm(absence(120.0, 245.0), THRESHOLD).await, []);
    }

    /// The user id is not secret, so this is no defence against an attacker, but it
    /// keeps two people's machines on one tailnet from vouching for each other.
    #[tokio::test]
    async fn ignores_an_answer_about_someone_else() {
        let mac = signed_in(Arc::new(Impostor), &["hostess"]);
        assert_eq!(
            mac.confirm(absence(120.0, 245.0), THRESHOLD).await,
            [absence(120.0, 245.0)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn decides_alone_when_a_peer_is_silent() {
        let mac = signed_in(Arc::new(Silent), &["hostess"]);
        assert_eq!(
            mac.confirm(absence(120.0, 245.0), THRESHOLD).await,
            [absence(120.0, 245.0)]
        );
    }

    #[tokio::test]
    async fn asks_nobody_before_it_knows_whose_machine_it_is() {
        let direct = Arc::new(Direct::default());
        let mac = Arc::new(Peers::new(THRESHOLD, direct.clone()));
        mac.set_names(vec!["hostess".into()]);
        assert_eq!(
            mac.confirm(absence(120.0, 245.0), THRESHOLD).await,
            [absence(120.0, 245.0)]
        );
        assert_eq!(direct.asked.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn cleans_up_the_names_it_is_given() {
        let peers = Peers::new(THRESHOLD, Arc::new(Silent));
        peers.set_names(vec![
            " hostess ".into(),
            "".into(),
            "hostess".into(),
            "air".into(),
        ]);
        assert_eq!(peers.names(), ["hostess", "air"]);
    }
}

mod answering {
    use super::*;

    #[test]
    fn refuses_before_it_knows_whose_machine_it_is() {
        let peers = Peers::new(THRESHOLD, Arc::new(Silent));
        assert_eq!(
            peers.answer_presence(USER, at(0.0)),
            Err(Refusal::NotSignedIn)
        );
    }

    #[test]
    fn refuses_a_question_about_someone_else() {
        let peers = signed_in(Arc::new(Silent), &[]);
        assert_eq!(
            peers.answer_presence(USER + 1, at(0.0)),
            Err(Refusal::OtherUser)
        );
        assert_eq!(
            peers.receive_touched(Touched {
                user_id: USER + 1,
                entry_ids: vec![1]
            }),
            Err(Refusal::OtherUser)
        );
    }

    #[test]
    fn hands_a_notice_to_whoever_probes_harvest() {
        let peers = signed_in(Arc::new(Silent), &[]);
        let heard = Arc::new(Mutex::new(Vec::new()));
        let record = heard.clone();
        peers.on_touched(move |ids| record.lock().unwrap().extend(ids));
        peers
            .receive_touched(Touched {
                user_id: USER,
                entry_ids: vec![636708906],
            })
            .unwrap();
        assert_eq!(*heard.lock().unwrap(), [636708906]);
    }

    /// A request is refused unless it comes from the tailnet, whatever the body.
    #[test]
    fn refuses_a_request_from_outside_the_tailnet() {
        let peers = signed_in(Arc::new(Silent), &[]);
        peers.note_presence(at(0.0));
        let query = format!("since={}&user_id={USER}", at(0.0));
        let (status, _) = server::respond(
            &peers,
            &Method::GET,
            "/presence",
            Some(&query),
            b"",
            "192.168.1.20".parse().unwrap(),
        );
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, body) = server::respond(
            &peers,
            &Method::GET,
            "/presence",
            Some(&query),
            b"",
            "100.101.102.103".parse().unwrap(),
        );
        assert_eq!(status, StatusCode::OK);
        let answer: PresenceAnswer = serde_json::from_str(&body).unwrap();
        assert_eq!(
            answer,
            PresenceAnswer {
                user_id: USER,
                intervals: vec![span(0.0, 0.0)]
            }
        );
    }

    #[test]
    fn says_what_is_wrong_with_a_malformed_request() {
        let peers = signed_in(Arc::new(Silent), &[]);
        let tailnet: IpAddr = "100.101.102.103".parse().unwrap();
        let ask = |method: &Method, path: &str, query: Option<&str>, body: &[u8]| {
            server::respond(&peers, method, path, query, body, tailnet).0
        };
        assert_eq!(
            ask(&Method::GET, "/presence", Some("since=yesterday"), b""),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            ask(&Method::POST, "/touched", None, b"{"),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            ask(&Method::GET, "/entries", None, b""),
            StatusCode::NOT_FOUND
        );
    }

    /// Over a real connection: the listener refuses a machine that is not on the
    /// tailnet, here this machine itself over loopback.
    #[tokio::test]
    async fn refuses_a_connection_from_outside_the_tailnet() {
        let peers = signed_in(Arc::new(Silent), &[]);
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let serving = tokio::spawn(server::serve(listener, peers));
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!(
                "http://{address}/presence?since={}&user_id={USER}",
                at(0.0)
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 403);
        serving.abort();
    }

    /// A name that does not resolve into the tailnet is not asked at all.
    #[tokio::test]
    async fn will_not_ask_a_machine_off_the_tailnet() {
        let transport = HttpPeerTransport::new(PEER_PORT);
        let refused = transport.presence("localhost", USER, at(0.0)).await;
        assert!(
            matches!(refused, Err(PeerError::NotOnTailnet { .. })),
            "{refused:?}"
        );
    }
}
