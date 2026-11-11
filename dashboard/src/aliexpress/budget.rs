//! What the platform's one AliExpress app may spend: a number of calls a day,
//! over every shop and node. What a sale or a member asks for always goes.
//! The sweeps spend what that leaves, a call at a time, at a steady pace,
//! each from a lane of its own ([`Lane`]): a bucket that fills at this node's
//! part of the lane's calls, and that what sales and members spent on this
//! node empties, each lane by its part. A lane that wants more than its part
//! waits at its own floor; the others go on at theirs.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use timada_core::Share;

const SECS_PER_DAY: f64 = 86_400.0;

/// How many seconds of calls a lane keeps when it has nothing to read: a
/// little to go on with, not a burst against AliExpress's limits once there
/// is.
const SAVED_SECS: f64 = 60.0;

/// How many seconds of calls a sweep looks for at a time: what is due is not
/// worth looking for more often, and may change by then.
const BATCH_SECS: u64 = 300;

/// The most rows a sweep looks for at a time.
const MAX_BATCH: i64 = 100;

/// The longest a sweep sleeps before looking at the budget again: the nodes
/// up may have changed, and with them this node's part.
const LONGEST_WAIT: Duration = Duration::from_secs(60);

/// What the sweeps spend the day's calls on, each from a part of its own, so
/// that the parcels of a busy season never starve the products, nor the
/// products the open orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// The imported products read again for their price and stock.
    Products,
    /// The open orders read again when no push came for a while.
    Orders,
    /// The parcels of the orders shipped, for the shopper's order page.
    Parcels,
}

impl Lane {
    pub const ALL: [Lane; 3] = [Lane::Products, Lane::Orders, Lane::Parcels];

    /// The lane's part of the day's calls, in hundredths. Products take most:
    /// there are far more of them than of open orders and parcels, and what
    /// a shopper pays goes by them. The pushes read the open orders as they
    /// change, so their lane only catches a push that never came. The
    /// parcels are followed less and less as they go quiet
    /// ([`super::sync`]), so their part carries a season's shipments.
    pub const fn percent(self) -> u64 {
        match self {
            Lane::Products => 60,
            Lane::Orders => 10,
            Lane::Parcels => 30,
        }
    }

    fn part(self) -> f64 {
        self.percent() as f64 / 100.0
    }

    fn index(self) -> usize {
        match self {
            Lane::Products => 0,
            Lane::Orders => 1,
            Lane::Parcels => 2,
        }
    }
}

pub struct CallBudget {
    per_day: u64,
    /// Every call this node made, counted by the clients.
    calls: Arc<AtomicU64>,
    buckets: Mutex<Buckets>,
}

struct Buckets {
    /// One per lane. May go below zero: what sales spent, the lane waits
    /// for.
    tokens: [f64; 3],
    at: Instant,
    /// The calls already taken out of the lanes.
    seen: u64,
}

impl CallBudget {
    /// The budget of `per_day` calls for the whole platform.
    pub fn new(per_day: u64) -> Self {
        Self {
            per_day,
            calls: Arc::new(AtomicU64::new(0)),
            buckets: Mutex::new(Buckets {
                tokens: [1.0; 3],
                at: Instant::now(),
                seen: 0,
            }),
        }
    }

    /// Where the clients count their calls.
    pub fn calls(&self) -> Arc<AtomicU64> {
        self.calls.clone()
    }

    /// Whether the sweeps get anything at all.
    pub fn spends(&self) -> bool {
        self.per_day > 0
    }

    /// This node's part of the budget, in calls a second, over every lane.
    fn per_sec(&self, share: Share) -> f64 {
        self.per_day as f64 / SECS_PER_DAY / share.of.max(1) as f64
    }

    /// How many rows the lane's sweep looks for at a time.
    pub fn batch(&self, lane: Lane, share: Share) -> i64 {
        let calls = u128::from(self.per_day) * u128::from(lane.percent()) * u128::from(BATCH_SECS)
            / (100 * u128::from(SECS_PER_DAY as u64) * u128::from(share.of.max(1)));
        i64::try_from(calls).unwrap_or(i64::MAX).clamp(1, MAX_BATCH)
    }

    /// Takes out a call of `lane` at `now` if there is one, and its time is
    /// paid for here; otherwise how long until there is. Every lane fills up
    /// to `now` first, and what sales and members spent since the last look
    /// comes out of each by its part.
    fn reserve(&self, lane: Lane, share: Share, now: Instant) -> Option<Duration> {
        let per_sec = self.per_sec(share);
        let mut buckets = self.buckets.lock().expect("buckets poisoned");
        let calls = self.calls.load(Ordering::Relaxed);
        let spent = calls.saturating_sub(buckets.seen) as f64;
        buckets.seen = buckets.seen.max(calls);
        let elapsed = now.saturating_duration_since(buckets.at).as_secs_f64();
        buckets.at = now;
        for each in Lane::ALL {
            let rate = per_sec * each.part();
            let saved = (rate * SAVED_SECS).max(1.0);
            // A burst of sales costs a lane at most a day.
            let floor = -(rate * SECS_PER_DAY);
            let tokens = &mut buckets.tokens[each.index()];
            *tokens = (*tokens + elapsed * rate - spent * each.part()).clamp(floor, saved);
        }
        let rate = per_sec * lane.part();
        let tokens = &mut buckets.tokens[lane.index()];
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            // The call this pays for is not to be taken out again.
            buckets.seen += 1;
            None
        } else {
            Some(Duration::from_secs_f64((1.0 - *tokens) / rate))
        }
    }

    /// Waits until the lane may make a call, with this node's part as `share`
    /// says when asked. Only for a budget that [`spends`](Self::spends).
    pub async fn ready(&self, lane: Lane, share: impl Fn() -> Share) {
        while let Some(wait) = self.reserve(lane, share(), Instant::now()) {
            tokio::time::sleep(wait.min(LONGEST_WAIT)).await;
        }
    }

    /// [`ready`](Self::ready) for `calls` calls in a row.
    pub async fn ready_for(&self, lane: Lane, calls: usize, share: impl Fn() -> Share) {
        for _ in 0..calls.max(1) {
            self.ready(lane, &share).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 864,000 calls a day, ten a second: six for the products, one for the
    /// orders, three for the parcels.
    fn budget() -> CallBudget {
        CallBudget::new(864_000)
    }

    const ALONE: Share = Share::ALONE;

    fn secs(duration: Duration) -> f64 {
        (duration.as_secs_f64() * 1000.0).round() / 1000.0
    }

    #[test]
    fn a_sweep_goes_at_a_steady_pace() {
        let budget = budget();
        let start = budget.buckets.lock().unwrap().at;
        assert_eq!(
            budget.reserve(Lane::Orders, ALONE, start),
            None,
            "one to start with"
        );
        assert_eq!(
            budget.reserve(Lane::Orders, ALONE, start),
            Some(Duration::from_secs(1))
        );
        let later = start + Duration::from_secs(1);
        assert_eq!(budget.reserve(Lane::Orders, ALONE, later), None);
        assert_eq!(
            budget.reserve(Lane::Orders, ALONE, later),
            Some(Duration::from_secs(1))
        );
    }

    #[test]
    fn each_lane_fills_at_its_part() {
        let budget = budget();
        let start = budget.buckets.lock().unwrap().at;
        assert_eq!(budget.reserve(Lane::Products, ALONE, start), None);
        assert_eq!(budget.reserve(Lane::Parcels, ALONE, start), None);
        let products = budget.reserve(Lane::Products, ALONE, start).unwrap();
        let parcels = budget.reserve(Lane::Parcels, ALONE, start).unwrap();
        assert_eq!(secs(products), 0.167, "six a second");
        assert_eq!(secs(parcels), 0.333, "three a second");
    }

    #[test]
    fn a_node_holds_its_part_of_the_calls() {
        let budget = budget();
        let start = budget.buckets.lock().unwrap().at;
        let third = Share { index: 1, of: 3 };
        assert_eq!(budget.reserve(Lane::Orders, third, start), None);
        assert_eq!(
            budget.reserve(Lane::Orders, third, start),
            Some(Duration::from_secs(3))
        );
    }

    #[test]
    fn what_a_node_saves_while_idle_is_no_burst() {
        let budget = budget();
        let start = budget.buckets.lock().unwrap().at;
        // Nothing to read for a day: a minute of calls saved, no more.
        let later = start + Duration::from_secs(86_400);
        for _ in 0..60 {
            assert_eq!(budget.reserve(Lane::Orders, ALONE, later), None);
        }
        assert!(budget.reserve(Lane::Orders, ALONE, later).is_some());
    }

    #[test]
    fn a_small_budget_still_makes_a_call_when_its_time_comes() {
        // Fifty calls a day, and three nodes: five for the orders, one every
        // 51,840 seconds.
        let budget = CallBudget::new(50);
        let start = budget.buckets.lock().unwrap().at;
        let third = Share { index: 0, of: 3 };
        assert_eq!(budget.reserve(Lane::Orders, third, start), None);
        let wait = budget.reserve(Lane::Orders, third, start).unwrap();
        assert_eq!(wait.as_secs(), 51_840);
        assert_eq!(budget.reserve(Lane::Orders, third, start + wait), None);
    }

    #[test]
    fn what_sales_spend_comes_out_of_every_lane_by_its_part() {
        let budget = budget();
        let start = budget.buckets.lock().unwrap().at;
        assert_eq!(budget.reserve(Lane::Orders, ALONE, start), None);

        // The sweep's read, and a hundred at payments, on this node: ten
        // of those for the orders' lane, sixty for the products'.
        budget.calls().fetch_add(101, Ordering::Relaxed);

        // They count from what there was: nothing left, and it takes that
        // much longer. The call the lane paid for is not taken out twice.
        let orders = budget.reserve(Lane::Orders, ALONE, start).unwrap();
        assert_eq!(orders, Duration::from_secs(11));
        let products = budget.reserve(Lane::Products, ALONE, start).unwrap();
        assert_eq!(secs(products), 10.0, "sixty of six a second");
        assert_eq!(budget.reserve(Lane::Orders, ALONE, start + orders), None);
    }

    #[test]
    fn a_sweeps_own_call_is_taken_out_once() {
        let budget = budget();
        let start = budget.buckets.lock().unwrap().at;
        assert_eq!(budget.reserve(Lane::Orders, ALONE, start), None);

        // The read the sweep asked for is made.
        budget.calls().fetch_add(1, Ordering::Relaxed);

        assert_eq!(
            budget.reserve(Lane::Orders, ALONE, start),
            Some(Duration::from_secs(1))
        );
    }

    #[test]
    fn one_lane_at_its_floor_leaves_the_others_their_calls() {
        let budget = budget();
        let start = budget.buckets.lock().unwrap().at;
        // The parcels want far more than their part: their lane runs dry
        // and waits.
        assert_eq!(budget.reserve(Lane::Parcels, ALONE, start), None);
        assert!(budget.reserve(Lane::Parcels, ALONE, start).is_some());
        // Their calls are made, and counted.
        budget.calls().fetch_add(1, Ordering::Relaxed);

        // The products go on at their own pace: the one to start with, and
        // six more a second later.
        let later = start + Duration::from_secs(1);
        for _ in 0..7 {
            assert_eq!(budget.reserve(Lane::Products, ALONE, later), None);
        }
        assert!(budget.reserve(Lane::Products, ALONE, later).is_some());
    }

    #[test]
    fn a_burst_of_sales_costs_a_lane_at_most_a_day() {
        let budget = budget();
        let start = budget.buckets.lock().unwrap().at;
        budget.calls().fetch_add(10_000_000, Ordering::Relaxed);

        let wait = budget.reserve(Lane::Orders, ALONE, start).unwrap();

        assert_eq!(wait, Duration::from_secs(86_401));
    }

    #[test]
    fn a_sweep_looks_for_a_few_minutes_of_calls_at_a_time() {
        let budget = CallBudget::new(12_000);
        assert_eq!(budget.batch(Lane::Products, ALONE), 25);
        assert_eq!(budget.batch(Lane::Parcels, ALONE), 12);
        assert_eq!(budget.batch(Lane::Orders, ALONE), 4);
        assert_eq!(budget.batch(Lane::Products, Share { index: 0, of: 3 }), 8);
        assert_eq!(
            CallBudget::new(50).batch(Lane::Orders, ALONE),
            1,
            "at least one"
        );
        assert_eq!(
            CallBudget::new(100_000_000).batch(Lane::Orders, ALONE),
            100,
            "at most"
        );
    }

    #[test]
    fn no_budget_spends_nothing() {
        assert!(!CallBudget::new(0).spends());
        assert!(CallBudget::new(1).spends());
    }
}
