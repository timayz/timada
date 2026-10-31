//! How often a client may do something: a sliding window per key, kept in
//! this node's memory. With several nodes behind a balancer, each counts on
//! its own, so a client spread over N of them gets up to N times the limit.
//!
//! The policies built on it are how often a storefront may have a sign-in
//! code mailed ([`allow_shopper_code`]), how often it may hand in a message
//! from its contact form ([`allow_contact_message`]) or a sign-up to the
//! shop's news ([`allow_newsletter_signup`]), and how often a password may
//! be tried ([`allow_password_login`]).

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::config::{AuthConfig, StoreConfig};

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
        self.check_all(&[(key, limit)], window)
    }

    /// Whether every key may go once more, each under its own limit; only
    /// then is the pass counted, against all of them. A refusal by one key
    /// costs the others nothing.
    pub fn check_all(&self, keys: &[(&str, u32)], window: Duration) -> bool {
        self.check_all_at(keys, window, Instant::now())
    }

    #[cfg(test)]
    fn check_at(&self, key: &str, limit: u32, window: Duration, now: Instant) -> bool {
        self.check_all_at(&[(key, limit)], window, now)
    }

    fn check_all_at(&self, wanted: &[(&str, u32)], window: Duration, now: Instant) -> bool {
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
        let mut full = false;
        for &(key, limit) in wanted {
            let Some(hits) = keys.get_mut(key) else {
                full |= limit == 0;
                continue;
            };
            hits.window = window;
            while hits
                .at
                .front()
                .is_some_and(|first| now.duration_since(*first) >= window)
            {
                hits.at.pop_front();
            }
            full |= hits.at.len() >= limit as usize;
        }
        if full {
            return false;
        }
        for &(key, _) in wanted {
            keys.entry(key.to_owned())
                .or_insert_with(|| Hits {
                    window,
                    at: VecDeque::new(),
                })
                .at
                .push_back(now);
        }
        true
    }
}

/// Whether a storefront may have one more sign-in code mailed: under both
/// its key's limit and its shopper's, and counted against both only when
/// let through, so one shopper refused again and again does not use up the
/// key's codes for everyone else. The shopper's address is the one the
/// storefront reports (`shopper_ip`, the `X-Shopper-IP` header), else the
/// caller's (`peer`); a report that is not an address is ignored. Without
/// either, only the key's limit applies.
pub fn allow_shopper_code(
    limiter: &RateLimiter,
    limits: &StoreConfig,
    key_id: &str,
    tenant_id: &str,
    shopper_ip: Option<&str>,
    peer: Option<IpAddr>,
) -> bool {
    allow_from_storefront(
        limiter,
        "shopper-code",
        &StorefrontLimits {
            per_key: limits.shopper_codes_per_key,
            per_ip: limits.shopper_codes_per_ip,
            window: Duration::from_secs(limits.shopper_codes_window_secs),
        },
        Caller {
            key_id,
            tenant_id,
            shopper_ip,
            peer,
        },
    )
}

/// Whether a storefront may hand in one more message from its contact form,
/// the way [`allow_shopper_code`] decides for codes: under both its key's
/// limit and its visitor's, counted only when let through.
pub fn allow_contact_message(
    limiter: &RateLimiter,
    limits: &StoreConfig,
    key_id: &str,
    tenant_id: &str,
    shopper_ip: Option<&str>,
    peer: Option<IpAddr>,
) -> bool {
    allow_from_storefront(
        limiter,
        "contact",
        &StorefrontLimits {
            per_key: limits.contact_messages_per_key,
            per_ip: limits.contact_messages_per_ip,
            window: Duration::from_secs(limits.contact_messages_window_secs),
        },
        Caller {
            key_id,
            tenant_id,
            shopper_ip,
            peer,
        },
    )
}

/// Whether a storefront may hand in one more sign-up to the shop's news,
/// the way [`allow_shopper_code`] decides for codes: under both its key's
/// limit and its visitor's, counted only when let through.
pub fn allow_newsletter_signup(
    limiter: &RateLimiter,
    limits: &StoreConfig,
    key_id: &str,
    tenant_id: &str,
    shopper_ip: Option<&str>,
    peer: Option<IpAddr>,
) -> bool {
    allow_from_storefront(
        limiter,
        "newsletter",
        &StorefrontLimits {
            per_key: limits.newsletter_signups_per_key,
            per_ip: limits.newsletter_signups_per_ip,
            window: Duration::from_secs(limits.newsletter_signups_window_secs),
        },
        Caller {
            key_id,
            tenant_id,
            shopper_ip,
            peer,
        },
    )
}

/// Whether one more password may be tried for `email` from `peer`: under
/// `[auth]`'s limit for the browser's address and for the e-mail address
/// (counted from anywhere, so a password cannot be guessed from many
/// addresses either). Every attempt let through counts, right or wrong: the
/// check runs before the password is looked at, so a wrong guess costs the
/// same as a right one and the answer says nothing more. Without the
/// browser's address, only the e-mail's limit applies.
pub fn allow_password_login(
    limiter: &RateLimiter,
    limits: &AuthConfig,
    email: &str,
    peer: Option<IpAddr>,
) -> bool {
    let window = Duration::from_secs(limits.login_window_secs);
    let email = format!("login:email:{}", email.trim().to_ascii_lowercase());
    match peer {
        Some(ip) => {
            let ip = format!("login:ip:{ip}");
            limiter.check_all(
                &[
                    (email.as_str(), limits.login_attempts_per_email),
                    (ip.as_str(), limits.login_attempts_per_ip),
                ],
                window,
            )
        }
        None => limiter.check(&email, limits.login_attempts_per_email, window),
    }
}

/// The limits of one storefront policy.
struct StorefrontLimits {
    per_key: u32,
    per_ip: u32,
    window: Duration,
}

/// Who asks: the storefront's key at its shop, and the visitor behind it.
struct Caller<'a> {
    key_id: &'a str,
    tenant_id: &'a str,
    shopper_ip: Option<&'a str>,
    peer: Option<IpAddr>,
}

/// What the storefront policies share: a key counted under `what:key:…` and
/// the visitor, when their address is known, under `what:ip:…` at that shop.
fn allow_from_storefront(
    limiter: &RateLimiter,
    what: &str,
    limits: &StorefrontLimits,
    caller: Caller<'_>,
) -> bool {
    let key = format!("{what}:key:{}", caller.key_id);
    let ip = caller
        .shopper_ip
        .and_then(|value| value.trim().parse::<IpAddr>().ok())
        .or(caller.peer)
        .map(|ip| format!("{what}:ip:{}:{ip}", caller.tenant_id));
    match ip {
        Some(ip) => limiter.check_all(
            &[(key.as_str(), limits.per_key), (ip.as_str(), limits.per_ip)],
            limits.window,
        ),
        None => limiter.check(&key, limits.per_key, limits.window),
    }
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
            contact_messages_per_ip: per_ip,
            contact_messages_per_key: per_key,
            contact_messages_window_secs: 3600,
            newsletter_signups_per_ip: per_ip,
            newsletter_signups_per_key: per_key,
            newsletter_signups_window_secs: 3600,
        }
    }

    fn auth(per_ip: u32, per_email: u32) -> AuthConfig {
        AuthConfig {
            secret_key: "0123456789abcdef0123456789abcdef".into(),
            session_lifetime_secs: 3600,
            rp_name: "Timada".into(),
            providers: Default::default(),
            login_attempts_per_ip: per_ip,
            login_attempts_per_email: per_email,
            login_window_secs: 900,
        }
    }

    #[test]
    fn a_password_is_tried_so_many_times_per_address_and_per_account() {
        let limiter = RateLimiter::new();
        let limits = auth(5, 2);
        let ip: IpAddr = "203.0.113.7".parse().unwrap();
        let other: IpAddr = "203.0.113.8".parse().unwrap();
        // The account's limit: two tries, from anywhere, however spelt.
        assert!(allow_password_login(
            &limiter,
            &limits,
            "Ada@Example.org",
            Some(ip)
        ));
        assert!(allow_password_login(
            &limiter,
            &limits,
            "ada@example.org ",
            Some(other)
        ));
        assert!(!allow_password_login(
            &limiter,
            &limits,
            "ada@example.org",
            Some(ip)
        ));
        assert!(!allow_password_login(
            &limiter,
            &limits,
            "ada@example.org",
            None
        ));
        // Other accounts from the same address still go, up to the address's
        // limit: one pass was spent on Ada from it, the refusal cost nothing.
        assert!(allow_password_login(
            &limiter,
            &limits,
            "bob@example.org",
            Some(ip)
        ));
        assert!(allow_password_login(
            &limiter,
            &limits,
            "carol@example.org",
            Some(ip)
        ));
        assert!(allow_password_login(
            &limiter,
            &limits,
            "dave@example.org",
            Some(ip)
        ));
        assert!(allow_password_login(
            &limiter,
            &limits,
            "erin@example.org",
            Some(ip)
        ));
        assert!(
            !allow_password_login(&limiter, &limits, "frank@example.org", Some(ip)),
            "the fifth account from one address: the address is held"
        );
        assert!(
            allow_password_login(&limiter, &limits, "frank@example.org", Some(other)),
            "from another address it goes"
        );
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

    #[test]
    fn a_refused_shopper_does_not_use_up_the_key() {
        let limiter = RateLimiter::new();
        let limits = limits(1, 2);
        let ask = |ip: &str| allow_shopper_code(&limiter, &limits, "k1", "t1", Some(ip), None);
        assert!(ask("203.0.113.1"));
        for _ in 0..5 {
            assert!(!ask("203.0.113.1"));
        }
        assert!(ask("203.0.113.2"));
        assert!(!ask("203.0.113.3"));
    }

    #[test]
    fn messages_are_counted_apart_from_codes() {
        let limiter = RateLimiter::new();
        let one_each = limits(1, 100);
        let ip = Some("203.0.113.9");
        assert!(allow_contact_message(
            &limiter, &one_each, "k1", "t1", ip, None
        ));
        assert!(!allow_contact_message(
            &limiter, &one_each, "k1", "t1", ip, None
        ));
        // A visitor out of messages may still have a sign-in code mailed.
        assert!(allow_shopper_code(
            &limiter, &one_each, "k1", "t1", ip, None
        ));
        // And the key's own share holds whatever the visitors.
        let tight = limits(100, 1);
        assert!(allow_contact_message(
            &limiter,
            &tight,
            "k2",
            "t1",
            Some("203.0.113.1"),
            None
        ));
        assert!(!allow_contact_message(
            &limiter,
            &tight,
            "k2",
            "t1",
            Some("203.0.113.2"),
            None
        ));
    }
}
