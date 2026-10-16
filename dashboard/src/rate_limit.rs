//! How often a client may do something: a sliding window per key, kept in
//! this node's memory. With several nodes behind a balancer, each counts on
//! its own, so a client spread over N of them gets up to N times the limit.
//!
//! The one policy built on it so far is how often a storefront may have a
//! sign-in code mailed ([`allow_shopper_code`]).

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::config::StoreConfig;

/// Past this many keys, a call also forgets the keys nobody used within
/// their window, so the map does not grow with every client ever seen. The
/// next sweep waits for twice as many as were kept, so a busy node does not
/// sweep on every call.
const SWEEP_ABOVE: usize = 10_000;

/// The times each key was let through, oldest first.
#[derive(Default)]
pub struct RateLimiter {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    keys: HashMap<String, Hits>,
    /// Sweep once there are more keys than this; `SWEEP_ABOVE` at first.
    sweep_above: usize,
}

struct Hits {
    window: Duration,
    at: VecDeque<Instant>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `key` may go once more: fewer than `limit` times let through
    /// within the last `window`. A refusal is not counted, so a client that
    /// keeps knocking is let through again once its oldest pass ages out.
    pub fn check(&self, key: &str, limit: u32, window: Duration) -> bool {
        self.check_at(key, limit, window, Instant::now())
    }

    fn check_at(&self, key: &str, limit: u32, window: Duration, now: Instant) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let State { keys, sweep_above } = &mut *state;
        if keys.len() > (*sweep_above).max(SWEEP_ABOVE) {
            keys.retain(|_, hits| {
                hits.at
                    .back()
                    .is_some_and(|last| now.duration_since(*last) < hits.window)
            });
            *sweep_above = keys.len() * 2;
        }
        let hits = keys.entry(key.to_owned()).or_insert_with(|| Hits {
            window,
            at: VecDeque::new(),
        });
        hits.window = window;
        while hits
            .at
            .front()
            .is_some_and(|first| now.duration_since(*first) >= window)
        {
            hits.at.pop_front();
        }
        if hits.at.len() >= limit as usize {
            return false;
        }
        hits.at.push_back(now);
        true
    }
}

/// Whether a storefront may have one more sign-in code mailed: first under
/// its key's limit, then under its shopper's. The shopper's address is the
/// one the storefront reports (`shopper_ip`, the `X-Shopper-IP` header),
/// else the caller's (`peer`); a report that is not an address is ignored.
/// Without either, only the key's limit applies.
pub fn allow_shopper_code(
    limiter: &RateLimiter,
    limits: &StoreConfig,
    key_id: &str,
    tenant_id: &str,
    shopper_ip: Option<&str>,
    peer: Option<IpAddr>,
) -> bool {
    let window = Duration::from_secs(limits.shopper_codes_window_secs);
    if !limiter.check(
        &format!("shopper-code:key:{key_id}"),
        limits.shopper_codes_per_key,
        window,
    ) {
        return false;
    }
    let ip = shopper_ip
        .and_then(|value| value.trim().parse::<IpAddr>().ok())
        .or(peer);
    let Some(ip) = ip else {
        return true;
    };
    limiter.check(
        &format!("shopper-code:ip:{tenant_id}:{ip}"),
        limits.shopper_codes_per_ip,
        window,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    #[test]
    fn a_key_is_let_through_up_to_the_limit_within_the_window() {
        let limiter = RateLimiter::new();
        let start = Instant::now();
        for i in 0..3 {
            assert!(
                limiter.check_at("a", 3, MINUTE, start + Duration::from_secs(i)),
                "pass {i}"
            );
        }
        assert!(!limiter.check_at("a", 3, MINUTE, start + Duration::from_secs(10)));
        // The first pass ages out a minute after it was made, not after the
        // refusal.
        assert!(limiter.check_at("a", 3, MINUTE, start + MINUTE));
        assert!(!limiter.check_at("a", 3, MINUTE, start + MINUTE));
        assert!(limiter.check_at("a", 3, MINUTE, start + MINUTE + Duration::from_secs(1)));
    }

    #[test]
    fn keys_are_counted_apart() {
        let limiter = RateLimiter::new();
        let now = Instant::now();
        assert!(limiter.check_at("a", 1, MINUTE, now));
        assert!(!limiter.check_at("a", 1, MINUTE, now));
        assert!(limiter.check_at("b", 1, MINUTE, now));
    }

    #[test]
    fn idle_keys_are_forgotten_once_there_are_many() {
        let limiter = RateLimiter::new();
        let start = Instant::now();
        for i in 0..=SWEEP_ABOVE {
            limiter.check_at(&i.to_string(), 1, MINUTE, start);
        }
        limiter.check_at("late", 1, MINUTE, start + MINUTE);
        assert_eq!(limiter.state.lock().unwrap().keys.len(), 1);
    }

    fn limits(per_ip: u32, per_key: u32) -> StoreConfig {
        StoreConfig {
            shopper_codes_per_ip: per_ip,
            shopper_codes_per_key: per_key,
            shopper_codes_window_secs: 600,
        }
    }

    #[test]
    fn a_shopper_ip_is_held_to_its_limit_and_others_still_pass() {
        let limiter = RateLimiter::new();
        let limits = limits(2, 100);
        let ask = |ip: Option<&str>| allow_shopper_code(&limiter, &limits, "k1", "t1", ip, None);
        assert!(ask(Some("203.0.113.7")));
        assert!(ask(Some(" 203.0.113.7 ")));
        assert!(!ask(Some("203.0.113.7")));
        assert!(ask(Some("2001:db8::1")));
        // The same address at another shop is another shopper.
        assert!(allow_shopper_code(
            &limiter,
            &limits,
            "k2",
            "t2",
            Some("203.0.113.7"),
            None
        ));
    }

    #[test]
    fn an_unreadable_shopper_ip_falls_back_to_the_caller() {
        let limiter = RateLimiter::new();
        let limits = limits(1, 100);
        let peer = Some("198.51.100.1".parse().unwrap());
        assert!(allow_shopper_code(
            &limiter,
            &limits,
            "k1",
            "t1",
            Some("not an address"),
            peer
        ));
        assert!(!allow_shopper_code(
            &limiter, &limits, "k1", "t1", None, peer
        ));
        // Neither: only the key counts.
        assert!(allow_shopper_code(
            &limiter, &limits, "k1", "t1", None, None
        ));
    }

    #[test]
    fn a_key_is_held_to_its_limit_whatever_the_shoppers() {
        let limiter = RateLimiter::new();
        let limits = limits(10, 2);
        assert!(allow_shopper_code(
            &limiter,
            &limits,
            "k1",
            "t1",
            Some("203.0.113.1"),
            None
        ));
        assert!(allow_shopper_code(
            &limiter,
            &limits,
            "k1",
            "t1",
            Some("203.0.113.2"),
            None
        ));
        assert!(!allow_shopper_code(
            &limiter,
            &limits,
            "k1",
            "t1",
            Some("203.0.113.3"),
            None
        ));
        assert!(allow_shopper_code(
            &limiter,
            &limits,
            "k2",
            "t1",
            Some("203.0.113.3"),
            None
        ));
    }
}
