//! Time source abstraction so expiry logic can be tested without sleeping.

use chrono::{DateTime, Utc};
use std::sync::Mutex;
use std::time::Duration;

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A clock that only moves when told to.
pub struct ManualClock {
    now: Mutex<DateTime<Utc>>,
}

impl ManualClock {
    pub fn new(start: DateTime<Utc>) -> Self {
        Self { now: Mutex::new(start) }
    }

    pub fn advance(&self, d: Duration) {
        let mut g = self.now.lock().unwrap();
        *g += chrono::Duration::from_std(d).expect("duration in range");
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn manual_clock_advances() {
        let c = ManualClock::new(chrono::DateTime::from_timestamp(1_000, 0).unwrap());
        let t0 = c.now();
        c.advance(Duration::from_secs(90));
        assert_eq!((c.now() - t0).num_seconds(), 90);
    }

    #[test]
    fn system_clock_is_recent() {
        let d = (chrono::Utc::now() - SystemClock.now()).num_seconds().abs();
        assert!(d < 5);
    }
}
