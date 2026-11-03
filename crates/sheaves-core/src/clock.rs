//! Where the core learns the time, so the rules that depend on it can be tested.

use std::sync::Mutex;

use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::calendar_date::CalendarDate;

pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
    /// The zone that decides which day it is.
    fn time_zone(&self) -> TimeZone;

    fn today(&self) -> CalendarDate {
        CalendarDate::of(self.now(), &self.time_zone())
    }
}

/// The machine's own clock and time zone.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }

    fn time_zone(&self) -> TimeZone {
        TimeZone::system()
    }
}

/// A clock that moves only when told to.
pub struct ManualClock {
    now: Mutex<Timestamp>,
    zone: TimeZone,
}

impl ManualClock {
    pub fn new(now: Timestamp, zone: TimeZone) -> Self {
        Self {
            now: Mutex::new(now),
            zone,
        }
    }

    pub fn set(&self, now: Timestamp) {
        *self.now.lock().unwrap() = now;
    }

    pub fn advance(&self, seconds: f64) {
        let mut now = self.now.lock().unwrap();
        *now = crate::models::offset_by(*now, seconds);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        *self.now.lock().unwrap()
    }

    fn time_zone(&self) -> TimeZone {
        self.zone.clone()
    }
}
