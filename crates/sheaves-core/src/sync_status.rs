//! Whether Harvest and Sheaves agree yet, as the footer of each shell says it.
//!
//! The core decides which state applies and what the counts are. The shells
//! word it in the locale (a relative time such as "3 minutes ago" is theirs to
//! format) and colour the dot by [`SyncTone`].

use jiff::Timestamp;

use crate::tracker::{Connection, TrackerState};

/// A sync this recent reads "just now". A relative-time formatter would call a
/// sync that finished a moment ago "in 0 seconds", which reads like the future.
pub const JUST_NOW_SECONDS: i64 = 45;

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SyncStatus {
    /// No token yet.
    NotConnected,
    Connecting,
    /// Harvest cannot be reached; `queued` changes wait for it.
    Offline {
        queued: u32,
    },
    /// Online, with `pending` changes not yet acknowledged.
    Syncing {
        pending: u32,
    },
    /// Online, nothing waiting, and the last sync was under [`JUST_NOW_SECONDS`] ago.
    SyncedJustNow,
    /// Online, nothing waiting. `at` is the last sync, or `None` when this
    /// session has not recorded one.
    Synced {
        at: Option<Timestamp>,
    },
}

/// How the shell colours the dot beside the words.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SyncTone {
    /// In step with Harvest: green.
    Good,
    /// Working on it, or changes waiting: yellow.
    Working,
    /// Harvest cannot be reached: orange.
    Warning,
    /// Nothing to report yet: grey.
    Idle,
}

impl SyncStatus {
    pub fn of(
        connection: &Connection,
        pending_count: u32,
        last_synced_at: Option<Timestamp>,
        now: Timestamp,
    ) -> Self {
        match connection {
            Connection::NeedsCredentials => Self::NotConnected,
            Connection::Connecting => Self::Connecting,
            Connection::Offline { .. } => Self::Offline {
                queued: pending_count,
            },
            Connection::Online if pending_count > 0 => Self::Syncing {
                pending: pending_count,
            },
            Connection::Online => match last_synced_at {
                Some(at) if now.as_second() - at.as_second() < JUST_NOW_SECONDS => {
                    Self::SyncedJustNow
                }
                at => Self::Synced { at },
            },
        }
    }

    pub fn tone(&self) -> SyncTone {
        match self {
            Self::NotConnected => SyncTone::Idle,
            Self::Connecting | Self::Syncing { .. } => SyncTone::Working,
            Self::Offline { .. } => SyncTone::Warning,
            Self::SyncedJustNow | Self::Synced { .. } => SyncTone::Good,
        }
    }
}

impl TrackerState {
    pub fn sync_status(&self) -> SyncStatus {
        SyncStatus::of(
            &self.connection,
            self.pending_count,
            self.last_synced_at,
            self.now,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).unwrap()
    }

    fn online(pending: u32, synced: Option<i64>, now: i64) -> SyncStatus {
        SyncStatus::of(&Connection::Online, pending, synced.map(at), at(now))
    }

    #[test]
    fn no_token_is_not_connected_whatever_is_queued() {
        let status = SyncStatus::of(&Connection::NeedsCredentials, 3, Some(at(0)), at(10));
        assert_eq!(status, SyncStatus::NotConnected);
        assert_eq!(status.tone(), SyncTone::Idle);
    }

    #[test]
    fn connecting_is_working() {
        let status = SyncStatus::of(&Connection::Connecting, 2, None, at(10));
        assert_eq!(status, SyncStatus::Connecting);
        assert_eq!(status.tone(), SyncTone::Working);
    }

    #[test]
    fn offline_counts_what_is_queued() {
        let offline = Connection::Offline {
            reason: "no route".into(),
        };
        let none = SyncStatus::of(&offline, 0, Some(at(0)), at(10));
        let some = SyncStatus::of(&offline, 4, Some(at(0)), at(10));
        assert_eq!(none, SyncStatus::Offline { queued: 0 });
        assert_eq!(some, SyncStatus::Offline { queued: 4 });
        assert_eq!(some.tone(), SyncTone::Warning);
    }

    #[test]
    fn online_with_changes_waiting_is_syncing_however_recent_the_sync() {
        let status = online(2, Some(100), 101);
        assert_eq!(status, SyncStatus::Syncing { pending: 2 });
        assert_eq!(status.tone(), SyncTone::Working);
    }

    #[test]
    fn a_sync_under_forty_five_seconds_old_is_just_now() {
        assert_eq!(online(0, Some(100), 100), SyncStatus::SyncedJustNow);
        assert_eq!(online(0, Some(100), 144), SyncStatus::SyncedJustNow);
        assert_eq!(SyncStatus::SyncedJustNow.tone(), SyncTone::Good);
    }

    #[test]
    fn at_forty_five_seconds_the_time_is_given() {
        assert_eq!(
            online(0, Some(100), 145),
            SyncStatus::Synced { at: Some(at(100)) }
        );
    }

    #[test]
    fn a_clock_behind_the_sync_still_reads_just_now() {
        assert_eq!(online(0, Some(100), 90), SyncStatus::SyncedJustNow);
    }

    #[test]
    fn online_with_no_recorded_sync_is_plain_synced() {
        assert_eq!(online(0, None, 100), SyncStatus::Synced { at: None });
    }
}
