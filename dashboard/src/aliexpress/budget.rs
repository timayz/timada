//! What the platform's one AliExpress app may spend: a number of calls a day,
//! over every shop and node. What a sale or a member asks for always goes. The
//! sweep spends what that leaves, a call at a time, at a steady pace: a bucket
//! that fills at this node's part of the day's calls, and that every call on
//! this node empties, whatever asked for it.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use timada_aliexpress::Share;

const SECS_PER_DAY: f64 = 86_400.0;

/// How many seconds of calls a node keeps when it has nothing to read: a little
/// to go on with, not a burst against AliExpress's limits once there is.
const SAVED_SECS: f64 = 60.0;

/// How many seconds of calls the sweep looks for at a time: what is due is not
/// worth looking for more often, and may change by then.
const BATCH_SECS: f64 = 300.0;

/// The most links the sweep looks for at a time.
const MAX_BATCH: i64 = 100;

/// The longest the sweep sleeps before looking at the budget again: the nodes
/// up may have changed, and with them this node's part.
const LONGEST_WAIT: Duration = Duration::from_secs(60);

pub struct CallBudget {
    per_day: u64,
    /// Every call this node made, counted by the clients.
    calls: Arc<AtomicU64>,
    bucket: Mutex<Bucket>,
}

struct Bucket {
    /// May go below zero: what sales spent, the sweep waits for.
    tokens: f64,
    at: Instant,
    /// The calls already taken out of `tokens`.
    seen: u64,
}

impl CallBudget {
    /// The budget of `per_day` calls for the whole platform.
    pub fn new(per_day: u64) -> Self {
        Self {
            per_day,
            calls: Arc::new(AtomicU64::new(0)),
            bucket: Mutex::new(Bucket {
                tokens: 1.0,
                at: Instant::now(),
                seen: 0,
            }),
        }
    }

    /// Where the clients count their calls.
    pub fn calls(&self) -> Arc<AtomicU64> {
        self.calls.clone()
    }

    /// Whether the sweep gets anything at all.
    pub fn spends(&self) -> bool {
        self.per_day > 0
    }

    /// This node's part of the budget, in calls a second.
    fn per_sec(&self, share: Share) -> f64 {
        self.per_day as f64 / SECS_PER_DAY / share.of.max(1) as f64
    }

    /// How many links the sweep looks for at a time.
    pub fn batch(&self, share: Share) -> i64 {
        ((self.per_sec(share) * BATCH_SECS) as i64).clamp(1, MAX_BATCH)
    }

    /// Takes out a call for the sweep at `now` if there is one, and its time
    /// is paid for here; otherwise how long until there is.
    fn reserve(&self, share: Share, now: Instant) -> Option<Duration> {
        let per_sec = self.per_sec(share);
        let saved = (per_sec * SAVED_SECS).max(1.0);
        let mut bucket = self.bucket.lock().expect("bucket poisoned");
        let calls = self.calls.load(Ordering::Relaxed);
        let spent = calls.saturating_sub(bucket.seen);
        bucket.seen = bucket.seen.max(calls);
        let elapsed = now.saturating_duration_since(bucket.at).as_secs_f64();
        bucket.at = now;
        // A burst of sales costs the sweep at most a day.
        let floor = -(per_sec * SECS_PER_DAY);
        bucket.tokens = (bucket.tokens + elapsed * per_sec - spent as f64).clamp(floor, saved);
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            // The call this pays for is not to be taken out again.
            bucket.seen += 1;
            None
        } else {
            Some(Duration::from_secs_f64((1.0 - bucket.tokens) / per_sec))
        }
    }

    /// Waits until the sweep may make a call, with this node's part as `share`
    /// says when asked. Only for a budget that [`spends`](Self::spends).
    pub async fn ready(&self, share: impl Fn() -> Share) {
        while let Some(wait) = self.reserve(share(), Instant::now()) {
            tokio::time::sleep(wait.min(LONGEST_WAIT)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 86,400 calls a day: one a second.
    fn budget() -> CallBudget {
        CallBudget::new(86_400)
    }

    const ALONE: Share = Share::ALONE;

    #[test]
    fn the_sweep_goes_at_a_steady_pace() {
        let budget = budget();
        let start = budget.bucket.lock().unwrap().at;
        assert_eq!(budget.reserve(ALONE, start), None, "one to start with");
        assert_eq!(budget.reserve(ALONE, start), Some(Duration::from_secs(1)));
        let later = start + Duration::from_secs(1);
        assert_eq!(budget.reserve(ALONE, later), None);
        assert_eq!(budget.reserve(ALONE, later), Some(Duration::from_secs(1)));
    }

    #[test]
    fn a_node_holds_its_part_of_the_calls() {
        let budget = budget();
        let start = budget.bucket.lock().unwrap().at;
        let third = Share { index: 1, of: 3 };
        assert_eq!(budget.reserve(third, start), None);
        assert_eq!(budget.reserve(third, start), Some(Duration::from_secs(3)));
    }

    #[test]
    fn what_a_node_saves_while_idle_is_no_burst() {
        let budget = budget();
        let start = budget.bucket.lock().unwrap().at;
        // Nothing to read for a day: a minute of calls saved, no more.
        let later = start + Duration::from_secs(86_400);
        for _ in 0..60 {
            assert_eq!(budget.reserve(ALONE, later), None);
        }
        assert!(budget.reserve(ALONE, later).is_some());
    }

    #[test]
    fn a_small_budget_still_makes_a_call_when_its_time_comes() {
        // Fifty calls a day, and three nodes: one call every 5,184 seconds.
        let budget = CallBudget::new(50);
        let start = budget.bucket.lock().unwrap().at;
        let third = Share { index: 0, of: 3 };
        assert_eq!(budget.reserve(third, start), None);
        let wait = budget.reserve(third, start).unwrap();
        assert_eq!(wait.as_secs(), 5_184);
        assert_eq!(budget.reserve(third, start + wait), None);
    }

    #[test]
    fn what_sales_spend_comes_out_of_the_sweeps_calls() {
        let budget = budget();
        let start = budget.bucket.lock().unwrap().at;
        assert_eq!(budget.reserve(ALONE, start), None);

        // Ten reads at payments, on this node.
        budget.calls().fetch_add(10, Ordering::Relaxed);

        // They count from what there was: nothing left, and it takes that much
        // longer. The call the sweep paid for is not taken out twice.
        let wait = budget.reserve(ALONE, start).unwrap();
        assert_eq!(wait, Duration::from_secs(10));
        assert_eq!(budget.reserve(ALONE, start + wait), None);
    }

    #[test]
    fn the_sweeps_own_call_is_taken_out_once() {
        let budget = budget();
        let start = budget.bucket.lock().unwrap().at;
        assert_eq!(budget.reserve(ALONE, start), None);

        // The read the sweep asked for is made.
        budget.calls().fetch_add(1, Ordering::Relaxed);

        assert_eq!(budget.reserve(ALONE, start), Some(Duration::from_secs(1)));
    }

    #[test]
    fn a_burst_of_sales_costs_the_sweep_at_most_a_day() {
        let budget = budget();
        let start = budget.bucket.lock().unwrap().at;
        budget.calls().fetch_add(10_000_000, Ordering::Relaxed);

        let wait = budget.reserve(ALONE, start).unwrap();

        assert_eq!(wait, Duration::from_secs(86_401));
    }

    #[test]
    fn the_sweep_looks_for_a_few_minutes_of_calls_at_a_time() {
        assert_eq!(CallBudget::new(12_000).batch(ALONE), 41);
        assert_eq!(CallBudget::new(12_000).batch(Share { index: 0, of: 3 }), 13);
        assert_eq!(CallBudget::new(50).batch(ALONE), 1, "at least one");
        assert_eq!(CallBudget::new(100_000_000).batch(ALONE), 100, "at most");
    }

    #[test]
    fn no_budget_spends_nothing() {
        assert!(!CallBudget::new(0).spends());
        assert!(CallBudget::new(1).spends());
    }
}
