use std::path::PathBuf;
use std::sync::Arc;

use super::*;
use crate::budget::{BudgetBasis, BudgetFigure};
use crate::calendar_date::Weekday;
use crate::client::{Method, Transport};
use crate::clock::SystemClock;
use crate::credentials::MemoryCredentialStore;
use crate::models::{Reference, offset_by};
use crate::test_support::fixture::{self, today};
use crate::test_support::{AccountOptions, Route, RoutingTransport};

/// Scratch files for one tracker, so tests never touch the real cache.
struct Scratch {
    _directory: tempfile::TempDir,
    snapshot: PathBuf,
    queue: PathBuf,
}

fn scratch() -> Scratch {
    let directory = tempfile::tempdir().unwrap();
    Scratch {
        snapshot: directory.path().join("snapshot.json"),
        queue: directory.path().join("queue.json"),
        _directory: directory,
    }
}

fn tracker_with(
    credentials: Credentials,
    transport: Arc<dyn Transport>,
    scratch: &Scratch,
) -> TimeTracker {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    TimeTracker::new(
        HarvestClient::new(Some(credentials), transport, 0.0),
        Arc::new(MemoryCredentialStore::default()),
        SnapshotStore::new(&scratch.snapshot),
        MutationQueue::open(&scratch.queue, clock.clone()),
        clock,
    )
}

fn tracker(transport: Arc<dyn Transport>, scratch: &Scratch) -> TimeTracker {
    tracker_with(fixture::credentials(), transport, scratch)
}

fn reopened_queue(scratch: &Scratch) -> MutationQueue {
    MutationQueue::open(&scratch.queue, Arc::new(SystemClock))
}

/// Today's fixture entry, renumbered and moved onto the project the fixture
/// account's only target belongs to.
fn existing_entry_for_the_target() -> String {
    fixture::time_entry()
        .replace("636709355", "700000001")
        .replace("14307913", "14308069")
}

/// Today's fixture entry, renumbered and moved to yesterday.
fn yesterdays_entry() -> String {
    fixture::time_entry()
        .replace("636709355", "700000002")
        .replace(&today().to_string(), &today().adding_days(-1).to_string())
}

fn with_entries(entries: Vec<String>) -> Arc<RoutingTransport> {
    RoutingTransport::account(AccountOptions {
        entries: Some(entries),
        ..AccountOptions::default()
    })
}

mod time_tracker {
    use super::*;

    #[tokio::test]
    async fn sync_populates_state() {
        let scratch = scratch();
        let tracker = tracker(RoutingTransport::standard_account(), &scratch);

        tracker.sync().await;

        let state = tracker.state();
        assert_eq!(state.connection, Connection::Online);
        assert_eq!(state.user.unwrap().name(), "Kim Allen");
        assert_eq!(state.company.unwrap().week_start_day, Weekday::Monday);
        assert_eq!(state.entries.len(), 1);
        assert_eq!(state.targets.len(), 1);
        assert!(state.last_synced_at.is_some());
    }

    /// Project assignments cost a request each sync and change about never; a burst
    /// of start and stop clicks should not re-download them.
    #[tokio::test]
    async fn caches_account_data() {
        let scratch = scratch();
        let transport = RoutingTransport::standard_account();
        let tracker = tracker(transport.clone(), &scratch);

        tracker.sync().await;
        tracker.sync().await;
        tracker.sync().await;

        assert_eq!(transport.call_count("project_assignments"), 1);
        assert_eq!(transport.call_count("company"), 1);
        // Two reads per sync (the day, and whatever is running), plus the one-off
        // history fetch that ranks the target list.
        assert_eq!(transport.call_count("time_entries"), 7);
    }

    #[tokio::test]
    async fn sync_keeps_readable_budgets() {
        // The running-timer account is on the one project the report budgets, which
        // is the case the panel actually draws.
        let scratch = scratch();
        let tracker = tracker(RoutingTransport::account_with_running_timer(), &scratch);

        tracker.sync().await;

        let state = tracker.state();
        assert_eq!(state.budgets.len(), 1);
        let running = state.running_entry().unwrap();
        assert_eq!(running.project.id, 14308069);
        assert_eq!(
            state.budget_for(running).unwrap().budget_remaining,
            Some(8.0)
        );
        // The project with no budget and the one whose figures came back null are
        // both absent, so nothing downstream has to tell them apart.
        assert!(!state.budgets.contains_key(&14307913));
        assert!(!state.budgets.contains_key(&14307915));
    }

    /// The Reports API allows 100 requests per 15 minutes, not per 15 seconds. A
    /// burst of start and stop clicks must not spend that allowance.
    #[tokio::test]
    async fn caches_budgets() {
        let scratch = scratch();
        let transport = RoutingTransport::standard_account();
        let tracker = tracker(transport.clone(), &scratch);

        tracker.sync().await;
        tracker.sync().await;
        tracker.sync().await;

        assert_eq!(transport.call_count("project_budget"), 1);
    }

    /// There is no lesser version of a budget display, so a token that may not read
    /// the report switches the feature off rather than leaving an empty frame. A
    /// refusal is the one settled answer: unlike an account that merely budgets
    /// nothing yet, it is never asked again.
    #[tokio::test]
    async fn probes_budgets_once() {
        let scratch = scratch();
        let transport = RoutingTransport::account(AccountOptions {
            budget_status: 403,
            ..AccountOptions::default()
        });
        let tracker = tracker(transport.clone(), &scratch);

        tracker.sync().await;
        tracker.sync().await;
        tracker.sync().await;

        assert_eq!(transport.call_count("project_budget"), 1);
        assert!(tracker.state().budgets.is_empty());
        // A refused report is not a broken sync.
        assert_eq!(tracker.state().connection, Connection::Online);
    }

    #[tokio::test]
    async fn account_without_budgets() {
        let scratch = scratch();
        let transport = RoutingTransport::account(AccountOptions {
            budgets: fixture::NO_PROJECT_BUDGETS_PAGE.to_string(),
            ..AccountOptions::default()
        });
        let tracker = tracker(transport, &scratch);

        tracker.sync().await;

        let state = tracker.state();
        assert!(state.budgets.is_empty());
        assert!(state.budget_for(&state.entries[0]).is_none());
        assert_eq!(state.connection, Connection::Online);
    }

    /// A budget is a decoration. Failing to load one must not tell the user their
    /// time tracking is offline, and must not stop Sheaves trying again.
    #[tokio::test]
    async fn budget_failure_is_harmless() {
        let scratch = scratch();
        let transport = RoutingTransport::account(AccountOptions {
            budget_status: 500,
            ..AccountOptions::default()
        });
        let tracker = tracker(transport.clone(), &scratch);

        tracker.sync().await;
        let state = tracker.state();
        assert_eq!(state.connection, Connection::Online);
        assert!(state.budgets.is_empty());
        assert_eq!(state.entries.len(), 1);

        // Transient, so unlike a refusal it is retried.
        let first_run = transport.call_count("project_budget");
        tracker.sync().await;
        assert!(transport.call_count("project_budget") > first_run);
    }

    #[tokio::test]
    async fn restores_budgets_from_snapshot() {
        let scratch = scratch();
        let transport = RoutingTransport::standard_account();
        tracker(transport.clone(), &scratch).sync().await;

        let second = self::scratch();
        let restored = TimeTracker::new(
            HarvestClient::new(Some(fixture::credentials()), transport, 0.0),
            Arc::new(MemoryCredentialStore::default()),
            SnapshotStore::new(&scratch.snapshot),
            MutationQueue::open(&second.queue, Arc::new(SystemClock)),
            Arc::new(SystemClock),
        );
        restored.bootstrap().await;

        assert_eq!(
            restored.state().budgets[&14308069].budget_remaining,
            Some(8.0)
        );
    }

    /// The whole point of the local-first model: the timer shows up even though
    /// Harvest never hears about it.
    #[tokio::test]
    async fn start_is_optimistic() {
        let scratch = scratch();
        let transport = RoutingTransport::standard_account();
        let tracker = tracker(transport.clone(), &scratch);
        tracker.sync().await;
        let target = tracker.state().targets[0].clone();
        transport.go_offline();

        tracker.start(&target, Some("spike")).await;

        let state = tracker.state();
        let running = state.running_entry().unwrap();
        assert_eq!(running.task.name, target.task.name);
        assert_eq!(running.notes.as_deref(), Some("spike"));
        assert!(running.is_pending);
        assert_eq!(running.id.server_id(), None);
        assert_eq!(state.recent_targets[0].id(), target.id());
        assert_eq!(state.pending_count, 1);
    }

    #[tokio::test]
    async fn start_reconciles_with_server() {
        let scratch = scratch();
        let transport = RoutingTransport::standard_account();
        let tracker = tracker(transport.clone(), &scratch);
        tracker.sync().await;
        let target = tracker.state().targets[0].clone();

        tracker.start(&target, None).await;

        // Nothing is left owing, and no local placeholder survives the refresh.
        let state = tracker.state();
        assert_eq!(state.pending_count, 0);
        assert!(
            state
                .entries
                .iter()
                .all(|entry| entry.id.server_id().is_some())
        );
        // Assert the create actually happened. Counting requests would pass even if
        // the mutation had been silently discarded, since every sync issues GETs.
        let creates = transport.calls_to(Method::Post, "time_entries");
        assert_eq!(creates.len(), 1);
        assert_eq!(creates[0].project_id, Some(target.project.id));
    }

    /// Harvest allows one running timer per user; the UI must not imply otherwise.
    #[tokio::test]
    async fn start_stops_the_previous_timer() {
        let scratch = scratch();
        let transport = RoutingTransport::account_with_running_timer();
        let tracker = tracker(transport.clone(), &scratch);
        tracker.sync().await;
        assert!(tracker.state().running_entry().is_some());
        let target = tracker.state().targets[0].clone();
        transport.go_offline();

        tracker.start(&target, None).await;

        let state = tracker.state();
        assert_eq!(
            state
                .entries
                .iter()
                .filter(|entry| entry.is_running)
                .count(),
            1
        );
        assert_eq!(state.running_entry().unwrap().task.name, target.task.name);
    }

    /// "Start on Today" must resume an entry the user already has today, not add a
    /// duplicate, which is why the offer goes to today before starting: the dedupe
    /// in `start` can only see the visible day's list.
    #[tokio::test]
    async fn offer_resumes_existing_today_entry() {
        let scratch = scratch();
        let transport = with_entries(vec![existing_entry_for_the_target()]);
        let tracker = tracker(transport.clone(), &scratch);
        tracker.sync().await;
        let target = tracker.state().targets[0].clone();
        tracker.select_day(today().adding_days(-1)).await;

        // The dialog's action: to today first, then start.
        tracker.go_to_today().await;
        tracker.start(&target, Some("carried over")).await;

        assert!(transport.calls_to(Method::Post, "time_entries").is_empty());
        assert_eq!(
            transport.calls_to(Method::Patch, "700000001/restart").len(),
            1
        );
    }

    /// Stopping a today-dated timer while viewing a past day must leave it one click
    /// away in the menu bar, not strand a stale running copy.
    #[tokio::test]
    async fn off_panel_stop_stays_recent() {
        let scratch = scratch();
        let transport = RoutingTransport::account_with_running_timer();
        let tracker = tracker(transport.clone(), &scratch);
        tracker.sync().await;
        // The running timer is dated today; browse to yesterday, where it is folded
        // into the visible list, and stop it from there.
        tracker.select_day(today().adding_days(-1)).await;
        transport.go_offline();
        let running = tracker.state().running_entry().cloned().unwrap();

        tracker.stop(&running).await;

        let Activity::Recent(entry) = tracker.state().activity() else {
            panic!(
                "expected a recent activity, got {:?}",
                tracker.state().activity()
            );
        };
        assert_eq!(entry.spent_date, today());
        assert!(!entry.is_running);
    }

    /// The offer's weak spot: offline, going to today clears the list and the failed
    /// sync leaves it empty, so the dedupe is blind. The tracker still remembers
    /// today's last entry, and that is enough to resume, not duplicate.
    #[tokio::test]
    async fn offer_falls_back_to_remembered_entry_offline() {
        let scratch = scratch();
        let transport = with_entries(vec![existing_entry_for_the_target()]);
        let tracker = tracker(transport.clone(), &scratch);
        tracker.sync().await;
        let target = tracker.state().targets[0].clone();
        tracker.select_day(today().adding_days(-1)).await;
        transport.go_offline();

        tracker.go_to_today().await;
        tracker.start(&target, Some("carried over")).await;

        assert_eq!(
            tracker.state().running_entry().map(|entry| entry.id),
            Some(EntryId::Server(700000001))
        );
        assert!(transport.calls_to(Method::Post, "time_entries").is_empty());
    }

    /// Genuinely resuming a past day's entry must not evict what the menu bar knows
    /// about today: stopping that timer should fall back to offering today's own last
    /// entry, not collapse to idle around a stale running copy.
    #[tokio::test]
    async fn past_day_timer_leaves_today_activity_alone() {
        let scratch = scratch();
        let transport = with_entries(vec![fixture::time_entry(), yesterdays_entry()]);
        let tracker = tracker(transport.clone(), &scratch);
        tracker.sync().await;
        let past = tracker
            .state()
            .entries
            .into_iter()
            .find(|entry| entry.id == EntryId::Server(700000002))
            .unwrap();
        transport.go_offline();

        tracker.resume(&past).await;
        let running = tracker.state().running_entry().cloned().unwrap();
        tracker.stop(&running).await;

        // Whether it clears the recency window depends on the entry's age; what must
        // hold is that today's entry survived and no running copy is stuck.
        let remembered = tracker.state().last_active_today.unwrap();
        assert_eq!(remembered.id, EntryId::Server(636709355));
        assert_eq!(remembered.spent_date, today());
        assert!(!remembered.is_running);
    }

    /// Resuming a past-day entry implicitly stops whatever today timer is running.
    /// That stop is an update: a timer that had run for hours must not drop out of
    /// the recency window the moment something else starts.
    #[tokio::test]
    async fn implicit_stop_stays_recent() {
        let scratch = scratch();
        // The fixture's running timer started long before any recency window.
        let transport = with_entries(vec![fixture::running_time_entry(), yesterdays_entry()]);
        let tracker = tracker(transport.clone(), &scratch);
        tracker.sync().await;
        let past = tracker
            .state()
            .entries
            .into_iter()
            .find(|entry| entry.id == EntryId::Server(700000002))
            .unwrap();
        transport.go_offline();

        tracker.resume(&past).await;
        let running = tracker.state().running_entry().cloned().unwrap();
        tracker.stop(&running).await;

        let Activity::Recent(entry) = tracker.state().activity() else {
            panic!(
                "expected a recent activity, got {:?}",
                tracker.state().activity()
            );
        };
        assert_eq!(entry.id, EntryId::Server(636708906));
        assert!(!entry.is_running);
    }

    /// Switching timers must send the old one's measured stop, in order, ahead of the
    /// new create. Leaning on Harvest stopping it implicitly banks the old timer up
    /// to whenever the create lands, inflating it by however long the network was
    /// away.
    #[tokio::test]
    async fn switch_queues_stop_before_create() {
        let scratch = scratch();
        let running = fixture::entries_page(&[fixture::running_time_entry()]);
        let transport = RoutingTransport::new(vec![
            Route::new(
                Method::Get,
                "reports/project_budget",
                fixture::PROJECT_BUDGET_PAGE,
            ),
            Route::new(Method::Get, "v2/clients", fixture::CLIENTS_PAGE),
            Route::new(
                Method::Get,
                "users/me/project_assignments",
                fixture::PROJECT_ASSIGNMENTS_PAGE,
            ),
            Route::new(Method::Get, "users/me", fixture::CURRENT_USER),
            Route::new(Method::Get, "company", fixture::COMPANY),
            Route::new(Method::Patch, "/stop", &fixture::time_entry()),
            Route::new(Method::Patch, "/restart", &fixture::running_time_entry()),
            Route::new(Method::Post, "time_entries", &fixture::running_time_entry()).status(201),
            Route::new(Method::Get, "is_running=true", &running),
            Route::new(Method::Get, "time_entries", &running),
        ]);
        let tracker = tracker(transport.clone(), &scratch);
        tracker.sync().await;
        let target = tracker.state().targets[0].clone();
        assert!(tracker.state().running_entry().is_some());

        tracker.start(&target, None).await;

        let stop = transport.first_index(Method::Patch, "/stop").unwrap();
        let create = transport.first_index(Method::Post, "time_entries").unwrap();
        assert!(
            stop < create,
            "the measured stop must land before the new create"
        );
        // The correction carries the total measured here, not Harvest's arrival math.
        let correction = transport
            .calls_to(Method::Patch, "time_entries")
            .into_iter()
            .find(|call| !call.path.contains("/stop") && !call.path.contains("/restart"))
            .unwrap();
        assert!(correction.hours.unwrap() >= 1.0);
    }

    /// Started, stopped and resumed, all before the queue drained. The create keeps
    /// the first run, and the resume goes as a restart, so the pause between them is
    /// never billed.
    #[tokio::test]
    async fn a_pause_before_the_first_drain_is_not_billed() {
        let scratch = scratch();
        let transport = RoutingTransport::standard_account();
        let tracker = tracker(transport.clone(), &scratch);
        tracker.sync().await;
        let target = tracker.state().targets[0].clone();
        transport.go_offline();

        tracker.start(&target, None).await;
        let running = tracker.state().running_entry().cloned().unwrap();
        tracker.stop(&running).await;
        let stopped = tracker
            .state()
            .entries
            .into_iter()
            .find(|entry| entry.id == running.id)
            .unwrap();
        tracker.resume(&stopped).await;

        let pending = reopened_queue(&scratch).pending();
        assert!(matches!(
            pending[0],
            Mutation::Create {
                ended_at: Some(_),
                ..
            }
        ));
        assert!(matches!(pending[1], Mutation::Restart { .. }));
    }

    #[tokio::test]
    async fn stop_banks_elapsed_time() {
        let scratch = scratch();
        let transport = RoutingTransport::account_with_running_timer();
        let tracker = tracker(transport.clone(), &scratch);
        tracker.sync().await;
        let entry = tracker.state().running_entry().cloned().unwrap();
        transport.go_offline();

        tracker.stop(&entry).await;

        let state = tracker.state();
        let stopped = state
            .entries
            .iter()
            .find(|candidate| candidate.id == entry.id)
            .unwrap();
        assert!(!stopped.is_running);
        assert_eq!(stopped.timer_started_at, None);
        // The fixture banked an hour before the timer was started.
        assert!(stopped.banked_hours >= 1.0);
    }

    #[tokio::test]
    async fn start_resumes_matching_entry() {
        let scratch = scratch();
        let transport = RoutingTransport::standard_account();
        let tracker = tracker(transport.clone(), &scratch);
        tracker.sync().await;
        let existing = tracker.state().entries[0].clone();
        let before = tracker.state().entries.len();
        transport.go_offline();

        tracker.start(&existing.target(), None).await;

        let state = tracker.state();
        assert_eq!(state.entries.len(), before);
        assert_eq!(
            state.running_entry().map(|entry| entry.id),
            Some(existing.id)
        );
    }

    #[tokio::test]
    async fn stays_usable_offline() {
        let scratch = scratch();
        // Every route is missing.
        let tracker = tracker(RoutingTransport::new(Vec::new()), &scratch);

        tracker.sync().await;

        assert_ne!(tracker.state().connection, Connection::Online);
    }

    #[tokio::test]
    async fn queues_undeliverable_changes() {
        let scratch = scratch();
        let transport = RoutingTransport::standard_account();
        let tracker = tracker(transport.clone(), &scratch);
        tracker.sync().await;
        let target = tracker.state().targets[0].clone();
        transport.go_offline();

        tracker.start(&target, None).await;
        assert_eq!(tracker.state().pending_count, 1);

        // A queue that never drains would make offline support a lie.
        let reconnected = RoutingTransport::standard_account();
        let recovered = self::tracker(reconnected.clone(), &scratch);
        recovered.sync().await;

        assert_eq!(recovered.state().pending_count, 0);
        // The queued start must have been *sent*, not dropped: the pending count
        // reaching zero is also what discarding it would look like.
        assert_eq!(reconnected.calls_to(Method::Post, "time_entries").len(), 1);
    }

    #[tokio::test]
    async fn select_day_reloads() {
        let scratch = scratch();
        let tracker = tracker(RoutingTransport::standard_account(), &scratch);
        tracker.sync().await;

        tracker.shift_day(-1).await;

        assert_eq!(tracker.state().day, today().adding_days(-1));
        assert!(!tracker.state().is_today());
        tracker.go_to_today().await;
        assert!(tracker.state().is_today());
    }

    #[tokio::test]
    async fn totals_the_day() {
        let scratch = scratch();
        let tracker = tracker(RoutingTransport::standard_account(), &scratch);

        tracker.sync().await;

        assert_eq!(tracker.state().total_hours(), 2.11);
    }

    #[tokio::test]
    async fn reports_decoding_failures() {
        let scratch = scratch();
        let transport =
            RoutingTransport::new(vec![Route::new(Method::Get, "users/me", r#"{"id": 1}"#)]);
        let tracker = tracker(transport, &scratch);

        tracker.sync().await;

        let Connection::Offline { reason } = tracker.state().connection else {
            panic!(
                "expected an offline connection, got {:?}",
                tracker.state().connection
            );
        };
        assert!(reason.contains("first_name"), "{reason}");
        assert!(reason.contains("users/me"), "{reason}");
    }
}

/// The shells draw from these snapshots alone, so every change has to reach them.
mod observing {
    use super::*;
    use std::sync::Mutex;

    #[tokio::test]
    async fn delivers_each_change_as_a_whole_state() {
        let scratch = scratch();
        let tracker = tracker(RoutingTransport::standard_account(), &scratch);
        let seen: Arc<Mutex<Vec<TrackerState>>> = Arc::default();
        let sink = seen.clone();
        tracker.observe(move |state| sink.lock().unwrap().push(state));

        tracker.sync().await;

        let seen = seen.lock().unwrap();
        // The current state arrives at once, before anything changes.
        assert_eq!(seen[0].connection, Connection::NeedsCredentials);
        // And the last one delivered is the state the tracker ended in.
        assert_eq!(seen.last(), Some(&tracker.state()));
        assert_eq!(seen.last().unwrap().connection, Connection::Online);
    }
}

mod disconnecting {
    use super::*;
    use crate::test_support::Held;

    /// A sync whose refresh is still in flight when the user disconnects must not
    /// land afterwards: it would put the old account's entries back on screen, and
    /// into the cache the next launch restores.
    #[tokio::test]
    async fn waits_out_a_sync_in_flight() {
        let scratch = scratch();
        let transport = Held::new(RoutingTransport::standard_account(), "time_entries");
        let tracker = tracker(transport.clone(), &scratch);
        tracker.sync().await;

        transport.hold();
        let syncing = {
            let tracker = tracker.clone();
            tokio::spawn(async move { tracker.sync().await })
        };
        while transport.held() == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        let disconnecting = {
            let tracker = tracker.clone();
            tokio::spawn(async move { tracker.disconnect().await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        transport.release();
        syncing.await.unwrap();
        disconnecting.await.unwrap();

        let state = tracker.state();
        assert_eq!(state.connection, Connection::NeedsCredentials);
        assert!(state.entries.is_empty());
        assert!(state.user.is_none());
        assert!(SnapshotStore::new(&scratch.snapshot).load().is_none());
    }
}

/// A token revoked while changes wait offline: the tracker asks for new
/// credentials, and keeps the changes for them.
mod rejected_token {
    use super::*;

    #[tokio::test]
    async fn asks_for_credentials_and_keeps_the_queue() {
        let scratch = scratch();
        let transport = RoutingTransport::standard_account();
        let tracker = tracker(transport.clone(), &scratch);
        tracker.sync().await;
        let target = tracker.state().targets[0].clone();
        transport.go_offline();
        tracker.start(&target, None).await;

        let revoked = RoutingTransport::new(vec![
            Route::new(Method::Post, "time_entries", "").status(401),
            // Who the token speaks for is still readable, so the drain runs and meets
            // the rejection on the change itself.
            Route::new(Method::Get, "users/me", fixture::CURRENT_USER),
        ]);
        let reopened = self::tracker(revoked, &scratch);
        reopened.sync().await;

        assert_eq!(reopened.state().connection, Connection::NeedsCredentials);
        assert_eq!(reopened.state().pending_count, 1);
        assert_eq!(reopened_queue(&scratch).count(), 1);
    }
}

mod budget_currencies {
    use super::*;

    /// An account budgeting only in hours never needs a currency, so it must not
    /// spend a request, and a 403 on clients is exactly what a regular user gets.
    #[tokio::test]
    async fn skips_currency_for_hours_budgets() {
        let scratch = scratch();
        let transport = RoutingTransport::standard_account();
        let tracker = tracker(transport.clone(), &scratch);

        tracker.sync().await;

        // The fixture's only readable budget is measured in hours.
        assert_eq!(
            tracker.state().budgets[&14308069].budget_by,
            BudgetBasis::Project
        );
        assert_eq!(transport.call_count("v2/clients"), 0);
    }

    #[tokio::test]
    async fn joins_currency() {
        let scratch = scratch();
        let transport = RoutingTransport::account(AccountOptions {
            budgets: fixture::MONETARY_BUDGET_PAGE.to_string(),
            ..AccountOptions::default()
        });
        let tracker = tracker(transport.clone(), &scratch);

        tracker.sync().await;

        // Client 5735776 bills in EUR, whatever the machine running this is set to.
        assert_eq!(
            tracker.state().budgets[&14308069].currency_code.as_deref(),
            Some("EUR")
        );
        assert_eq!(transport.call_count("v2/clients"), 1);
    }

    /// The permission for clients is nearly the one a monetary budget already
    /// needs, but not exactly, so the refusal has to leave the budget usable.
    #[tokio::test]
    async fn survives_refused_clients() {
        let scratch = scratch();
        let transport = RoutingTransport::account(AccountOptions {
            budgets: fixture::MONETARY_BUDGET_PAGE.to_string(),
            clients: "{}".to_string(),
            client_status: 403,
            ..AccountOptions::default()
        });
        let tracker = tracker(transport.clone(), &scratch);

        tracker.sync().await;

        let state = tracker.state();
        assert_eq!(state.connection, Connection::Online);
        let budget = &state.budgets[&14308069];
        assert_eq!(budget.currency_code, None);
        assert_eq!(budget.budget_remaining, Some(8.0));
        assert_eq!(
            budget.remaining(),
            Some(BudgetFigure::Money {
                amount: 8.0,
                currency: None
            })
        );

        // A refusal is a settled answer, so it is not asked again.
        tracker.sync().await;
        assert_eq!(transport.call_count("v2/clients"), 1);
    }
}

mod reconnecting {
    use super::*;

    /// A 403 on the budget report is treated as final, which is right for one token
    /// and wrong across two. An expired token drops the connection to "needs
    /// credentials" without `disconnect` being called, so a session could reconnect
    /// with a better token and still never ask again.
    #[tokio::test]
    async fn connect_resets_refusals() {
        let scratch = scratch();
        // First token: allowed to track time, refused the budget report.
        let refusing = RoutingTransport::account(AccountOptions {
            budget_status: 403,
            ..AccountOptions::default()
        });
        let tracker = tracker(refusing.clone(), &scratch);
        tracker.sync().await;
        tracker.sync().await;
        assert!(tracker.state().budgets.is_empty());
        assert_eq!(refusing.call_count("project_budget"), 1);

        tracker
            .connect(fixture::credentials(), false)
            .await
            .unwrap();

        // The same scripted account still refuses, but the probe was asked again
        // rather than being written off with the previous token.
        assert_eq!(refusing.call_count("project_budget"), 2);
    }

    /// Resetting the probe without clearing what it gates was the worse half of a
    /// fix: a budget refresh only overwrites on success, so a transient failure after
    /// connecting left the previous account's figures on screen under the new token.
    #[tokio::test]
    async fn connect_clears_budgets() {
        let scratch = scratch();
        // A cache standing in for the previous account, so there is something to
        // inherit.
        SnapshotStore::new(&scratch.snapshot)
            .save(&CachedSnapshot {
                budgets: vec![ProjectBudget {
                    currency_code: Some("GBP".to_string()),
                    ..ProjectBudget::new(
                        14308069,
                        "Someone else's project",
                        BudgetBasis::ProjectCost,
                        Some(5000.0),
                        Some(100.0),
                        Some(4900.0),
                    )
                }],
                ..CachedSnapshot::default()
            })
            .unwrap();

        // New credentials, and a budget report that fails transiently rather than
        // answering: the case where nothing would overwrite what was cached.
        let transport = RoutingTransport::account(AccountOptions {
            budget_status: 500,
            ..AccountOptions::default()
        });
        let tracker = tracker(transport, &scratch);
        tracker.bootstrap().await;
        assert_eq!(
            tracker.state().budgets[&14308069].currency_code.as_deref(),
            Some("GBP"),
            "the cache should have been restored"
        );

        tracker
            .connect(fixture::credentials(), false)
            .await
            .unwrap();

        assert!(tracker.state().budgets.is_empty());
        assert_eq!(tracker.state().connection, Connection::Online);
    }
}

/// An expired token drops the connection without `disconnect` running, so whatever
/// was queued offline is still there when the next credentials arrive. Whose they
/// are decides everything: the same person must not lose their work, and anyone else
/// must not send it.
mod queue_ownership {
    use super::*;

    fn other_account() -> Credentials {
        Credentials::new("67890", "other")
    }

    fn colleague() -> Arc<RoutingTransport> {
        RoutingTransport::account(AccountOptions {
            user: fixture::CURRENT_USER.replace("1782959", "4242"),
            ..AccountOptions::default()
        })
    }

    /// Syncs as the fixture account, then starts a timer with the network gone, so
    /// one create is left queued under that account and user.
    async fn queue_one_start_offline(scratch: &Scratch) {
        let transport = RoutingTransport::standard_account();
        let tracker = tracker(transport.clone(), scratch);
        tracker.sync().await;
        let target = tracker.state().targets[0].clone();
        transport.go_offline();
        tracker.start(&target, None).await;
        assert_eq!(tracker.state().pending_count, 1);
    }

    #[tokio::test]
    async fn same_owner_keeps_queue() {
        let scratch = scratch();
        queue_one_start_offline(&scratch).await;

        let transport = RoutingTransport::standard_account();
        let tracker = tracker(transport.clone(), &scratch);
        // A new token for the same account: only the account and user identify it.
        tracker
            .connect(
                Credentials::new(fixture::credentials().account_id, "renewed"),
                false,
            )
            .await
            .unwrap();

        assert_eq!(tracker.state().pending_count, 0);
        assert_eq!(transport.calls_to(Method::Post, "time_entries").len(), 1);
    }

    #[tokio::test]
    async fn other_account_is_refused() {
        let scratch = scratch();
        queue_one_start_offline(&scratch).await;

        let transport = RoutingTransport::standard_account();
        let tracker = tracker(transport.clone(), &scratch);
        assert_eq!(
            tracker.connect(other_account(), false).await,
            Err(ConnectError::UnsentChangesBelongElsewhere { count: 1 })
        );

        assert_eq!(tracker.state().connection, Connection::NeedsCredentials);
        assert!(transport.calls_to(Method::Post, "time_entries").is_empty());
        // Still on disk for the account that made it to come back to.
        assert_eq!(reopened_queue(&scratch).count(), 1);
    }

    /// One account holds many people, and an administrator's token can write to a
    /// colleague's entries: the case where a stale mutation succeeds.
    #[tokio::test]
    async fn other_user_is_refused() {
        let scratch = scratch();
        queue_one_start_offline(&scratch).await;

        let transport = colleague();
        let tracker = tracker(transport.clone(), &scratch);
        assert_eq!(
            tracker.connect(fixture::credentials(), false).await,
            Err(ConnectError::UnsentChangesBelongElsewhere { count: 1 })
        );

        assert!(transport.calls_to(Method::Post, "time_entries").is_empty());
        assert_eq!(reopened_queue(&scratch).count(), 1);
    }

    #[tokio::test]
    async fn discarding_connects() {
        let scratch = scratch();
        queue_one_start_offline(&scratch).await;

        let transport = RoutingTransport::standard_account();
        let tracker = tracker(transport.clone(), &scratch);
        tracker.connect(other_account(), true).await.unwrap();

        assert_eq!(tracker.state().connection, Connection::Online);
        assert_eq!(tracker.state().pending_count, 0);
        assert!(transport.calls_to(Method::Post, "time_entries").is_empty());
        assert_eq!(
            reopened_queue(&scratch)
                .owner()
                .map(|owner| owner.account_id),
            Some("67890".to_string())
        );
    }

    /// Clearing the client on the way out turned a declined switch into a sign-out:
    /// the session that was working a moment ago could no longer sync at all.
    #[tokio::test]
    async fn refused_connect_changes_nothing() {
        let scratch = scratch();
        queue_one_start_offline(&scratch).await;

        let transport = RoutingTransport::standard_account();
        let tracker = tracker(transport.clone(), &scratch);
        assert!(matches!(
            tracker.connect(other_account(), false).await,
            Err(ConnectError::UnsentChangesBelongElsewhere { .. })
        ));
        tracker.sync().await;

        // The queued start went out after all, under the account that made it.
        let posts = transport.calls_to(Method::Post, "time_entries");
        let accounts: Vec<Option<String>> = posts.into_iter().map(|call| call.account_id).collect();
        assert_eq!(accounts, [Some(fixture::credentials().account_id)]);
        assert_eq!(tracker.state().connection, Connection::Online);
    }

    /// `connect` is not the only way to a drain: a launch that finds new credentials
    /// in the store beside a queue nobody claimed for them syncs straight away.
    #[tokio::test]
    async fn sync_refuses_another_owners_queue() {
        let scratch = scratch();
        queue_one_start_offline(&scratch).await;

        let transport = RoutingTransport::standard_account();
        let tracker = tracker_with(other_account(), transport.clone(), &scratch);
        tracker.sync().await;

        assert!(transport.calls_to(Method::Post, "time_entries").is_empty());
        assert_eq!(tracker.state().connection, Connection::NeedsCredentials);
        assert_eq!(tracker.state().pending_count, 1);
        assert_eq!(reopened_queue(&scratch).count(), 1);
    }

    #[tokio::test]
    async fn sync_refuses_another_users_queue() {
        let scratch = scratch();
        queue_one_start_offline(&scratch).await;
        // No cached user, so the sync has to ask Harvest who it is speaking for.
        std::fs::remove_file(&scratch.snapshot).ok();

        let transport = colleague();
        let tracker = tracker(transport.clone(), &scratch);
        tracker.sync().await;

        assert!(transport.calls_to(Method::Post, "time_entries").is_empty());
        assert_eq!(tracker.state().connection, Connection::NeedsCredentials);
    }

    /// The same, with the previous user still in the cache: a launch after dying
    /// between saving new credentials and claiming the queue.
    #[tokio::test]
    async fn sync_refuses_another_users_queue_despite_the_cache() {
        let scratch = scratch();
        queue_one_start_offline(&scratch).await;
        assert!(
            SnapshotStore::new(&scratch.snapshot)
                .load()
                .unwrap()
                .user
                .is_some()
        );

        let transport = colleague();
        let tracker = tracker(transport.clone(), &scratch);
        // Restores the cached user. The store holds no credentials, so it syncs
        // nothing itself; the client was given the colleague's token directly.
        tracker.bootstrap().await;
        assert_eq!(tracker.state().user.map(|user| user.id), Some(1782959));
        tracker.sync().await;

        assert!(transport.calls_to(Method::Post, "time_entries").is_empty());
        assert_eq!(tracker.state().connection, Connection::NeedsCredentials);
    }

    #[tokio::test]
    async fn unowned_queue_is_kept() {
        let scratch = scratch();
        let target = TimerTarget {
            project: Reference::new(14308069, "Online Store - Phase 1"),
            task: Reference::new(8083366, "Programming"),
            client: Some(Reference::new(5735776, "123 Industries")),
        };
        reopened_queue(&scratch).enqueue(Mutation::Create {
            local: Uuid::new_v4(),
            target,
            spent_date: today(),
            notes: None,
            started_at: offset_by(Timestamp::now(), -3600.0),
            ended_at: Some(Timestamp::now()),
        });

        let transport = RoutingTransport::standard_account();
        let tracker = tracker(transport.clone(), &scratch);
        tracker
            .connect(fixture::credentials(), false)
            .await
            .unwrap();

        assert_eq!(tracker.state().pending_count, 0);
        assert_eq!(transport.calls_to(Method::Post, "time_entries").len(), 1);
        assert_eq!(
            reopened_queue(&scratch).owner().map(|owner| owner.user_id),
            Some(1782959)
        );
    }
}

/// The background probe exists to catch changes made outside this app; these pin
/// how its cadence follows power and recent usage rather than the exact numbers,
/// which are tuning.
mod probe_cadence {
    use super::*;

    fn entry(running: bool) -> TrackedEntry {
        TrackedEntry {
            id: EntryId::Server(1),
            project: Reference::new(1, "P"),
            task: Reference::new(2, "T"),
            client: None,
            notes: None,
            spent_date: today(),
            banked_hours: if running { 0.0 } else { 0.5 },
            is_running: running,
            timer_started_at: running.then(Timestamp::now),
            is_locked: false,
            is_billable: true,
            is_pending: false,
            updated_at: Timestamp::now(),
        }
    }

    fn running() -> Activity {
        Activity::Running(entry(true))
    }

    #[test]
    fn low_power_stops_probing() {
        assert_eq!(probe_interval(&running(), PowerState::LowPower), None);
        assert_eq!(probe_interval(&Activity::Idle, PowerState::LowPower), None);
    }

    #[test]
    fn battery_slows_but_keeps_probing() {
        let active_mains = probe_interval(&running(), PowerState::PluggedIn).unwrap();
        let active_battery = probe_interval(&running(), PowerState::Battery).unwrap();
        let idle_mains = probe_interval(&Activity::Idle, PowerState::PluggedIn).unwrap();
        let idle_battery = probe_interval(&Activity::Idle, PowerState::Battery).unwrap();
        assert!(active_battery > active_mains);
        assert!(idle_battery > idle_mains);
    }

    #[test]
    fn recent_usage_quickens_probing() {
        for power in [PowerState::PluggedIn, PowerState::Battery] {
            let active = probe_interval(&running(), power).unwrap();
            let idle = probe_interval(&Activity::Idle, power).unwrap();
            assert!(active < idle);
        }
    }

    #[test]
    fn recent_timer_counts_as_usage() {
        let recent = Activity::Recent(entry(false));
        let interval = probe_interval(&recent, PowerState::PluggedIn).unwrap();
        let idle = probe_interval(&Activity::Idle, PowerState::PluggedIn).unwrap();
        assert!(interval < idle);
    }
}

mod target_ranking {
    use super::*;

    fn day() -> CalendarDate {
        CalendarDate::new(2026, 8, 29)
    }

    /// Ids are derived from the names, as the Swift test derives them from hashes:
    /// distinct names give distinct projects and tasks.
    fn id_of(name: &str) -> i64 {
        name.bytes().fold(7i64, |hash, byte| {
            hash.wrapping_mul(31).wrapping_add(i64::from(byte))
        }) & 0xFFFF
    }

    fn entry(project: &str, task: &str, days_ago: i32) -> TimeEntry {
        let spent = day().adding_days(-days_ago);
        TimeEntry {
            id: id_of(&format!("{project}{task}{spent}")),
            spent_date: spent,
            user: None,
            client: Some(Reference::new(1, "A Client")),
            project: Reference::new(id_of(project), project),
            task: Reference::new(id_of(task), task),
            hours: 1.0,
            hours_without_timer: Some(1.0),
            notes: None,
            is_locked: false,
            locked_reason: None,
            is_billed: false,
            is_running: false,
            billable: true,
            timer_started_at: None,
            started_time: None,
            ended_time: None,
            created_at: Timestamp::now(),
            updated_at: Timestamp::now(),
        }
    }

    #[test]
    fn only_includes_used_targets() {
        let ranked = rank_by_usage(&[entry("Acme", "Programming", 1)], day());
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].task.name, "Programming");
    }

    /// The complaint that prompted this: an alphabetical list puts Business
    /// Development above the thing you do every day.
    #[test]
    fn habit_beats_one_offs() {
        let mut entries: Vec<TimeEntry> = (0..10)
            .map(|days| entry("Acme", "Programming", days))
            .collect();
        entries.push(entry("Acme", "Business Development", 80));
        let ranked = rank_by_usage(&entries, day());
        assert_eq!(ranked.first().unwrap().task.name, "Programming");
        assert_eq!(ranked.last().unwrap().task.name, "Business Development");
    }

    /// Frequency alone would keep a finished project pinned for weeks.
    #[test]
    fn recency_beats_raw_frequency() {
        let entries: Vec<TimeEntry> = (60..90)
            .map(|days| entry("Old", "Programming", days))
            .chain((0..5).map(|days| entry("Current", "Programming", days)))
            .collect();
        assert_eq!(rank_by_usage(&entries, day())[0].project.name, "Current");
    }

    /// Recency alone would let one stray entry outrank months of real work.
    #[test]
    fn one_stray_entry_does_not_win() {
        let mut entries: Vec<TimeEntry> = (0..20)
            .map(|days| entry("Steady", "Programming", days + 2))
            .collect();
        entries.push(entry("Stray", "Meeting", 0));
        assert_eq!(rank_by_usage(&entries, day())[0].project.name, "Steady");
    }

    #[test]
    fn is_stable() {
        let entries = vec![entry("A", "One", 3), entry("B", "Two", 3)];
        let reversed: Vec<TimeEntry> = entries.iter().rev().cloned().collect();
        let first: Vec<String> = rank_by_usage(&entries, day())
            .iter()
            .map(TimerTarget::id)
            .collect();
        let second: Vec<String> = rank_by_usage(&reversed, day())
            .iter()
            .map(TimerTarget::id)
            .collect();
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn ordered_targets_merges_sources() {
        let scratch = scratch();
        let tracker = tracker(RoutingTransport::standard_account(), &scratch);

        tracker.sync().await;

        let state = tracker.state();
        let ordered = state.ordered_targets();
        assert_eq!(ordered.len(), state.targets.len());
        let unique: HashSet<String> = ordered.iter().map(TimerTarget::id).collect();
        assert_eq!(unique.len(), ordered.len());
    }
}

/// The fixture timer starts at 14:30Z with an hour already banked, so an absence
/// from 15:00Z leaves exactly 1.5 hours worked. Fixed instants keep the arithmetic
/// out of the assertions.
mod resolving_an_absence {
    use super::*;

    fn instant(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn absence() -> Absence {
        Absence {
            began: instant("2026-08-29T15:00:00Z"),
            ended: instant("2026-08-29T15:30:00Z"),
        }
    }

    fn meeting() -> TimerTarget {
        TimerTarget {
            project: Reference::new(14308069, "Online Store - Phase 1"),
            task: Reference::new(8083366, "Programming"),
            client: Some(Reference::new(5735776, "123 Industries")),
        }
    }

    fn running_account() -> Arc<RoutingTransport> {
        let running = fixture::entries_page(&[fixture::running_time_entry()]);
        RoutingTransport::new(vec![
            Route::new(
                Method::Get,
                "users/me/project_assignments",
                fixture::PROJECT_ASSIGNMENTS_PAGE,
            ),
            Route::new(Method::Get, "users/me", fixture::CURRENT_USER),
            Route::new(Method::Get, "company", fixture::COMPANY),
            Route::new(Method::Patch, "/stop", &fixture::time_entry()),
            Route::new(Method::Patch, "/restart", &fixture::running_time_entry()),
            Route::new(Method::Post, "time_entries", &fixture::time_entry()).status(201),
            Route::new(Method::Patch, "time_entries", &fixture::time_entry()),
            Route::new(Method::Get, "is_running=true", &running),
            Route::new(Method::Get, "time_entries", &running),
        ])
    }

    async fn tracker_with_running_timer(
        transport: &Arc<RoutingTransport>,
        scratch: &Scratch,
    ) -> (TimeTracker, TrackedEntry) {
        let tracker = tracker(transport.clone(), scratch);
        tracker.sync().await;
        let running = tracker.state().running_entry().cloned().unwrap();
        (tracker, running)
    }

    /// The hours each correction carried, skipping `/stop` and `/restart`, which
    /// carry none.
    fn corrections(transport: &RoutingTransport) -> Vec<f64> {
        transport
            .calls_to(Method::Patch, "time_entries")
            .into_iter()
            .filter_map(|call| call.hours)
            .collect()
    }

    #[tokio::test]
    async fn keep_is_silent() {
        let scratch = scratch();
        let transport = running_account();
        let (tracker, running) = tracker_with_running_timer(&transport, &scratch).await;

        tracker
            .resolve(&absence(), &running, &AbsenceResolution::Keep)
            .await;

        assert!(transport.calls_to(Method::Patch, "time_entries").is_empty());
        assert!(transport.calls_to(Method::Post, "time_entries").is_empty());
    }

    #[tokio::test]
    async fn trim_and_stop_sends_trimmed_hours() {
        let scratch = scratch();
        let transport = running_account();
        let (tracker, running) = tracker_with_running_timer(&transport, &scratch).await;

        tracker
            .resolve(&absence(), &running, &AbsenceResolution::TrimAndStop)
            .await;

        assert_eq!(transport.calls_to(Method::Patch, "/stop").len(), 1);
        assert_eq!(corrections(&transport), [1.5]);
        assert!(transport.calls_to(Method::Patch, "/restart").is_empty());
    }

    /// Harvest's own copy of the timer is still running and still counting the
    /// absence, so continuing has to stop it, correct the total, and start it again.
    /// Leaving out the stop would let Harvest keep the hours nobody worked.
    #[tokio::test]
    async fn trim_and_continue_stops_corrects_restarts() {
        let scratch = scratch();
        let transport = running_account();
        let (tracker, running) = tracker_with_running_timer(&transport, &scratch).await;

        tracker
            .resolve(&absence(), &running, &AbsenceResolution::TrimAndContinue)
            .await;

        assert_eq!(transport.calls_to(Method::Patch, "/stop").len(), 1);
        // The first correction is the trimmed total. A second follows here only
        // because the fixture's return time is hours in the past, so the queue banks
        // the stretch since; in life the two are seconds apart.
        assert_eq!(corrections(&transport).first(), Some(&1.5));
        assert_eq!(transport.calls_to(Method::Patch, "/restart").len(), 1);
    }

    #[tokio::test]
    async fn trim_and_log_creates_the_idle_entry() {
        let scratch = scratch();
        let transport = running_account();
        let (tracker, running) = tracker_with_running_timer(&transport, &scratch).await;

        tracker
            .resolve(
                &absence(),
                &running,
                &AbsenceResolution::TrimAndLog(meeting()),
            )
            .await;

        let created = transport.calls_to(Method::Post, "time_entries").remove(0);
        assert_eq!(created.hours, Some(0.5));
        assert_eq!(created.project_id, Some(meeting().project.id));
        // The absence's own day. Taking it from the interrupted timer would put this
        // morning's meeting on yesterday's timesheet whenever a timer ran overnight.
        assert_eq!(
            created.spent_date,
            Some(absence().day(&jiff::tz::TimeZone::system()).to_string())
        );
        // The timer it was taken from keeps running.
        assert_eq!(transport.calls_to(Method::Patch, "/restart").len(), 1);
    }

    /// A timer this machine has never seen running while anyone was here, started on
    /// the web or a phone, must be left alone entirely.
    #[tokio::test]
    async fn refuses_timer_started_elsewhere() {
        let scratch = scratch();
        let transport = running_account();
        let (tracker, running) = tracker_with_running_timer(&transport, &scratch).await;

        let earlier = Absence {
            began: instant("2026-08-29T14:00:00Z"),
            ended: instant("2026-08-29T15:30:00Z"),
        };
        tracker
            .resolve(&earlier, &running, &AbsenceResolution::TrimAndStop)
            .await;

        assert!(transport.calls_to(Method::Patch, "/stop").is_empty());
        assert!(transport.calls_to(Method::Patch, "time_entries").is_empty());
        assert!(transport.calls_to(Method::Post, "time_entries").is_empty());
    }
}

/// A create still sitting in the queue reports its hours as the span from its
/// start, which silently swallows any pause taken before the queue drained.
/// Trimming one therefore sends the measured total explicitly rather than trusting
/// that span.
mod trimming_a_create_that_never_reached_harvest {
    use super::*;

    #[tokio::test]
    async fn corrects_the_span() {
        let scratch = scratch();
        let transport = RoutingTransport::new(vec![
            Route::new(Method::Post, "time_entries", &fixture::time_entry()).status(201),
            Route::new(Method::Patch, "time_entries", &fixture::time_entry()),
        ]);
        let client = HarvestClient::new(Some(fixture::credentials()), transport.clone(), 0.0);
        let queue = reopened_queue(&scratch);
        let local = Uuid::new_v4();
        let target = TimerTarget {
            project: Reference::new(14308069, "Online Store - Phase 1"),
            task: Reference::new(8083366, "Programming"),
            client: None,
        };

        // Worked 09:00 to 09:30, paused, back at 10:00, away from 10:20: fifty
        // minutes.
        let start = Timestamp::from_second(1_700_000_000).unwrap();
        queue.enqueue(Mutation::Create {
            local,
            target,
            spent_date: today(),
            notes: None,
            started_at: start,
            ended_at: None,
        });
        assert!(queue.amend_create(local, Some(offset_by(start, 80.0 * 60.0))));
        queue.enqueue(Mutation::Update {
            id: EntryId::Local(local),
            notes: None,
            hours: Some(50.0 / 60.0),
        });

        let report = queue.drain(&client).await;
        assert_eq!(report.applied, 2);

        // The create still reports the whole span, pause included, rounded to the
        // two decimals Harvest keeps…
        assert_eq!(
            transport.calls_to(Method::Post, "time_entries")[0].hours,
            Some(1.33)
        );
        // …so the total that stands has to be the one that was measured.
        let corrections: Vec<f64> = transport
            .calls_to(Method::Patch, "time_entries")
            .into_iter()
            .filter_map(|call| call.hours)
            .collect();
        assert_eq!(corrections, [0.83]);
    }
}
