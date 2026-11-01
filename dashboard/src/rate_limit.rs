//! How often a client may do something: a sliding window per key, held
//! across the cluster. Each node keeps in memory the passes it let through;
//! before letting one more through it asks the other nodes for theirs
//! (`POST /cluster/limits`, under `cluster.secret`) and counts them with its
//! own, so a client spread over a balancer is held to one limit, not one per
//! node. A node that does not answer in time counts for nothing: its share
//! is given up rather than the request held back. Two nodes deciding at the
//! very same moment may each let the last pass through, so the limit holds
//! to within a moment's requests, not to the unit. The keys travel hashed:
//! they name e-mail and network addresses.
//!
//! The policies built on it are how often a storefront may have a sign-in
//! code mailed ([`allow_shopper_code`]), how often it may hand in a message
//! from its contact form ([`allow_contact_message`]) or a sign-up to the
//! shop's news ([`allow_newsletter_signup`]), and how often a password may
//! be tried ([`allow_password_login`]).

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use openidconnect::reqwest;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{AuthConfig, StoreConfig};
use crate::peers::{self, Peers};

/// How long a node waits for a peer's counts before counting it for nothing.
const PEER_TIMEOUT: Duration = Duration::from_secs(2);

/// The most keys one question to a peer may carry: the policies ask about
/// two.
const MAX_KEYS_ASKED: usize = 16;

/// Past this many keys, a call also forgets the keys nobody used within
/// their window, so the map does not grow with every client ever seen. The
/// next sweep waits for twice as many as were kept, so a busy node does not
/// sweep on every call.
const SWEEP_ABOVE: usize = 10_000;

/// The times each key was let through on this node, oldest first, and the
/// other nodes to ask for theirs.
pub struct RateLimiter {
    state: Mutex<State>,
    /// The other nodes; `None` on a single node, which asks nobody.
    peers: Option<Arc<Peers>>,
}

/// What a node asks a peer: its passes of these keys within the window.
#[derive(Serialize, Deserialize)]
pub struct Asked {
    window_secs: u64,
    keys: Vec<String>,
}

/// A peer's answer: its passes of each key asked, in the order asked.
#[derive(Serialize, Deserialize)]
pub struct Counted {
    counts: Vec<u64>,
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
    /// A limiter asking `peers` for their counts; `None` for a single node.
    pub fn new(peers: Option<Arc<Peers>>) -> Self {
        Self {
            state: Mutex::default(),
            peers,
        }
    }

    /// Whether `key` may go once more: fewer than `limit` times let through
    /// within the last `window`, on this node and the others together. A
    /// refusal is not counted, so a client that keeps knocking is let
    /// through again once its oldest pass ages out.
    pub async fn check(&self, key: &str, limit: u32, window: Duration) -> bool {
        self.check_all(&[(key, limit)], window).await
    }

    /// Whether every key may go once more, each under its own limit; only
    /// then is the pass counted, against all of them. A refusal by one key
    /// costs the others nothing.
    pub async fn check_all(&self, keys: &[(&str, u32)], window: Duration) -> bool {
        let hashed: Vec<(String, u32)> = keys
            .iter()
            .map(|&(key, limit)| (digest(key), limit))
            .collect();
        let hashed: Vec<(&str, u32)> = hashed
            .iter()
            .map(|(key, limit)| (key.as_str(), *limit))
            .collect();
        let elsewhere = self.counts_elsewhere(&hashed, window).await;
        self.check_all_at(&hashed, window, Instant::now(), &elsewhere)
    }

    /// Whether a request carries the cluster's secret.
    pub fn authorizes(&self, header: Option<&[u8]>) -> bool {
        peers::authorizes(self.peers.as_deref(), header)
    }

    /// Answers a peer's question (the body of its `POST /cluster/limits`)
    /// with this node's own passes. `Err` for a body that is not a question.
    pub fn answer(&self, body: &[u8]) -> anyhow::Result<Counted> {
        let asked: Asked = serde_json::from_slice(body)?;
        anyhow::ensure!(
            asked.keys.len() <= MAX_KEYS_ASKED,
            "{} keys asked, at most {MAX_KEYS_ASKED} may be",
            asked.keys.len()
        );
        Ok(Counted {
            counts: self.counts_at(
                &asked.keys,
                Duration::from_secs(asked.window_secs),
                Instant::now(),
            ),
        })
    }

    /// The other nodes' passes of each key within `window`, summed per key
    /// in the order of `wanted`. A node that fails to answer in time counts
    /// for nothing, with a warning.
    async fn counts_elsewhere(&self, wanted: &[(&str, u32)], window: Duration) -> Vec<u64> {
        let mut elsewhere = vec![0u64; wanted.len()];
        let Some(peers) = self.peers.as_deref() else {
            return elsewhere;
        };
        let body = serde_json::to_vec(&Asked {
            window_secs: window.as_secs(),
            keys: wanted.iter().map(|&(key, _)| key.to_owned()).collect(),
        })
        .expect("a question serializes");
        let asks: Vec<_> = peers
            .urls()
            .iter()
            .map(|url| {
                let request = peers
                    .client()
                    .post(format!("{url}/cluster/limits"))
                    .header(reqwest::header::AUTHORIZATION, peers.authorization())
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .timeout(PEER_TIMEOUT)
                    .body(body.clone());
                let url = url.clone();
                tokio::spawn(async move { (url, ask(request).await) })
            })
            .collect();
        for ask in asks {
            let Ok((url, answered)) = ask.await else {
                continue;
            };
            match answered {
                Ok(counted) if counted.counts.len() == wanted.len() => {
                    for (total, count) in elsewhere.iter_mut().zip(counted.counts) {
                        *total = total.saturating_add(count);
                    }
                }
                Ok(counted) => tracing::warn!(
                    peer = %url,
                    asked = wanted.len(),
                    answered = counted.counts.len(),
                    "a peer answered for other keys than asked; its passes are not counted"
                ),
                Err(err) => tracing::warn!(
                    peer = %url,
                    %err,
                    "cannot ask a peer what it let through; its passes are not counted"
                ),
            }
        }
        elsewhere
    }

    /// This node's own passes of each key within `window`, in order.
    fn counts_at(&self, keys: &[String], window: Duration, now: Instant) -> Vec<u64> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        keys.iter()
            .map(|key| match state.keys.get(key) {
                Some(hits) => hits
                    .at
                    .iter()
                    .filter(|at| now.duration_since(**at) < window)
                    .count() as u64,
                None => 0,
            })
            .collect()
    }

    #[cfg(test)]
    fn check_at(&self, key: &str, limit: u32, window: Duration, now: Instant) -> bool {
        self.check_all_at(&[(key, limit)], window, now, &[])
    }

    /// The decision, from this node's own passes and the other nodes'
    /// (`elsewhere`, per key in the order of `wanted`; short means none).
    fn check_all_at(
        &self,
        wanted: &[(&str, u32)],
        window: Duration,
        now: Instant,
        elsewhere: &[u64],
    ) -> bool {
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
        for (index, &(key, limit)) in wanted.iter().enumerate() {
            let here = match keys.get_mut(key) {
                Some(hits) => {
                    hits.window = window;
                    while hits
                        .at
                        .front()
                        .is_some_and(|first| now.duration_since(*first) >= window)
                    {
                        hits.at.pop_front();
                    }
                    hits.at.len() as u64
                }
                None => 0,
            };
            let elsewhere = elsewhere.get(index).copied().unwrap_or(0);
            full |= here.saturating_add(elsewhere) >= u64::from(limit);
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

/// One peer's answer, or why there is none.
async fn ask(request: reqwest::RequestBuilder) -> anyhow::Result<Counted> {
    let response = request.send().await?;
    anyhow::ensure!(
        response.status().is_success(),
        "answered {}",
        response.status()
    );
    Ok(response.json().await?)
}

/// The key as it travels and is kept: its SHA-256, so a node never sends or
/// holds an e-mail or network address in the clear for this.
fn digest(key: &str) -> String {
    Sha256::digest(key.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            use std::fmt::Write;
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// Whether a storefront may have one more sign-in code mailed: under both
/// its key's limit and its shopper's, and counted against both only when
/// let through, so one shopper refused again and again does not use up the
/// key's codes for everyone else. The shopper's address is the one the
/// storefront reports (`shopper_ip`, the `X-Shopper-IP` header), else the
/// caller's (`peer`); a report that is not an address is ignored. Without
/// either, only the key's limit applies.
pub async fn allow_shopper_code(
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
    .await
}

/// Whether a storefront may hand in one more message from its contact form,
/// the way [`allow_shopper_code`] decides for codes: under both its key's
/// limit and its visitor's, counted only when let through.
pub async fn allow_contact_message(
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
    .await
}

/// Whether a storefront may hand in one more sign-up to the shop's news,
/// the way [`allow_shopper_code`] decides for codes: under both its key's
/// limit and its visitor's, counted only when let through.
pub async fn allow_newsletter_signup(
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
    .await
}

/// Whether one more password may be tried for `email` from `peer`: under
/// `[auth]`'s limit for the browser's address and for the e-mail address
/// (counted from anywhere, so a password cannot be guessed from many
/// addresses either). Every attempt let through counts, right or wrong: the
/// check runs before the password is looked at, so a wrong guess costs the
/// same as a right one and the answer says nothing more. Without the
/// browser's address, only the e-mail's limit applies.
pub async fn allow_password_login(
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
            limiter
                .check_all(
                    &[
                        (email.as_str(), limits.login_attempts_per_email),
                        (ip.as_str(), limits.login_attempts_per_ip),
                    ],
                    window,
                )
                .await
        }
        None => {
            limiter
                .check(&email, limits.login_attempts_per_email, window)
                .await
        }
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
async fn allow_from_storefront(
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
        Some(ip) => {
            limiter
                .check_all(
                    &[(key.as_str(), limits.per_key), (ip.as_str(), limits.per_ip)],
                    limits.window,
                )
                .await
        }
        None => limiter.check(&key, limits.per_key, limits.window).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peers::test_support::{SECRET, answering, down, peer};

    const MINUTE: Duration = Duration::from_secs(60);

    #[test]
    fn a_key_is_let_through_up_to_the_limit_within_the_window() {
        let limiter = RateLimiter::new(None);
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
        let limiter = RateLimiter::new(None);
        let now = Instant::now();
        assert!(limiter.check_at("a", 1, MINUTE, now));
        assert!(!limiter.check_at("a", 1, MINUTE, now));
        assert!(limiter.check_at("b", 1, MINUTE, now));
    }

    #[test]
    fn idle_keys_are_forgotten_once_there_are_many() {
        let limiter = RateLimiter::new(None);
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

    #[tokio::test]
    async fn a_password_is_tried_so_many_times_per_address_and_per_account() {
        let limiter = RateLimiter::new(None);
        let limits = auth(5, 2);
        let ip: IpAddr = "203.0.113.7".parse().unwrap();
        let other: IpAddr = "203.0.113.8".parse().unwrap();
        // The account's limit: two tries, from anywhere, however spelt.
        assert!(allow_password_login(&limiter, &limits, "Ada@Example.org", Some(ip)).await);
        assert!(allow_password_login(&limiter, &limits, "ada@example.org ", Some(other)).await);
        assert!(!allow_password_login(&limiter, &limits, "ada@example.org", Some(ip)).await);
        assert!(!allow_password_login(&limiter, &limits, "ada@example.org", None).await);
        // Other accounts from the same address still go, up to the address's
        // limit: one pass was spent on Ada from it, the refusal cost nothing.
        assert!(allow_password_login(&limiter, &limits, "bob@example.org", Some(ip)).await);
        assert!(allow_password_login(&limiter, &limits, "carol@example.org", Some(ip)).await);
        assert!(allow_password_login(&limiter, &limits, "dave@example.org", Some(ip)).await);
        assert!(allow_password_login(&limiter, &limits, "erin@example.org", Some(ip)).await);
        assert!(
            !allow_password_login(&limiter, &limits, "frank@example.org", Some(ip)).await,
            "the fifth account from one address: the address is held"
        );
        assert!(
            allow_password_login(&limiter, &limits, "frank@example.org", Some(other)).await,
            "from another address it goes"
        );
    }

    #[tokio::test]
    async fn a_shopper_ip_is_held_to_its_limit_and_others_still_pass() {
        let limiter = RateLimiter::new(None);
        let limits = limits(2, 100);
        let ask =
            |ip: Option<&'static str>| allow_shopper_code(&limiter, &limits, "k1", "t1", ip, None);
        assert!(ask(Some("203.0.113.7")).await);
        assert!(ask(Some(" 203.0.113.7 ")).await);
        assert!(!ask(Some("203.0.113.7")).await);
        assert!(ask(Some("2001:db8::1")).await);
        // The same address at another shop is another shopper.
        assert!(allow_shopper_code(&limiter, &limits, "k2", "t2", Some("203.0.113.7"), None).await);
    }

    #[tokio::test]
    async fn an_unreadable_shopper_ip_falls_back_to_the_caller() {
        let limiter = RateLimiter::new(None);
        let limits = limits(1, 100);
        let peer = Some("198.51.100.1".parse().unwrap());
        assert!(
            allow_shopper_code(&limiter, &limits, "k1", "t1", Some("not an address"), peer).await
        );
        assert!(!allow_shopper_code(&limiter, &limits, "k1", "t1", None, peer).await);
        // Neither: only the key counts.
        assert!(allow_shopper_code(&limiter, &limits, "k1", "t1", None, None).await);
    }

    #[tokio::test]
    async fn a_key_is_held_to_its_limit_whatever_the_shoppers() {
        let limiter = RateLimiter::new(None);
        let limits = limits(10, 2);
        assert!(allow_shopper_code(&limiter, &limits, "k1", "t1", Some("203.0.113.1"), None).await);
        assert!(allow_shopper_code(&limiter, &limits, "k1", "t1", Some("203.0.113.2"), None).await);
        assert!(
            !allow_shopper_code(&limiter, &limits, "k1", "t1", Some("203.0.113.3"), None).await
        );
        assert!(allow_shopper_code(&limiter, &limits, "k2", "t1", Some("203.0.113.3"), None).await);
    }

    #[tokio::test]
    async fn a_refused_shopper_does_not_use_up_the_key() {
        let limiter = RateLimiter::new(None);
        let limits = limits(1, 2);
        let ask =
            |ip: &'static str| allow_shopper_code(&limiter, &limits, "k1", "t1", Some(ip), None);
        assert!(ask("203.0.113.1").await);
        for _ in 0..5 {
            assert!(!ask("203.0.113.1").await);
        }
        assert!(ask("203.0.113.2").await);
        assert!(!ask("203.0.113.3").await);
    }

    #[tokio::test]
    async fn messages_are_counted_apart_from_codes() {
        let limiter = RateLimiter::new(None);
        let one_each = limits(1, 100);
        let ip = Some("203.0.113.9");
        assert!(allow_contact_message(&limiter, &one_each, "k1", "t1", ip, None).await);
        assert!(!allow_contact_message(&limiter, &one_each, "k1", "t1", ip, None).await);
        // A visitor out of messages may still have a sign-in code mailed.
        assert!(allow_shopper_code(&limiter, &one_each, "k1", "t1", ip, None).await);
        // And the key's own share holds whatever the visitors.
        let tight = limits(100, 1);
        assert!(
            allow_contact_message(&limiter, &tight, "k2", "t1", Some("203.0.113.1"), None).await
        );
        assert!(
            !allow_contact_message(&limiter, &tight, "k2", "t1", Some("203.0.113.2"), None).await
        );
    }

    #[tokio::test]
    async fn the_other_nodes_passes_count_too() {
        // Three nodes: a peer that let the key through twice, and one down.
        let limiter = RateLimiter::new(Some(Arc::new(Peers::for_test(
            vec![peer(200, br#"{"counts":[2]}"#).await, down().await],
            3,
        ))));
        assert!(limiter.check("a", 3, MINUTE).await);
        assert!(!limiter.check("a", 3, MINUTE).await);
        // Nothing let through here yet, the peer's two fill the limit alone.
        assert!(!limiter.check("b", 2, MINUTE).await);

        // A peer that fails, or answers for other keys than asked, counts
        // for nothing.
        let unsure = RateLimiter::new(Some(Arc::new(Peers::for_test(
            vec![
                peer(500, b"").await,
                peer(200, br#"{"counts":[5,5]}"#).await,
                peer(200, b"not json").await,
            ],
            5,
        ))));
        assert!(unsure.check("a", 1, MINUTE).await);
        assert!(!unsure.check("a", 1, MINUTE).await);
    }

    /// A node serving `POST /cluster/limits` from `limiter`, as the route
    /// does: the secret, then the answer.
    async fn node(limiter: Arc<RateLimiter>) -> String {
        answering(move |head, body| {
            let authorization = head
                .lines()
                .find_map(|line| line.strip_prefix("authorization: "))
                .map(str::as_bytes);
            if !limiter.authorizes(authorization) {
                return (401, Vec::new());
            }
            match limiter.answer(body) {
                Ok(counted) => (200, serde_json::to_vec(&counted).unwrap()),
                Err(_) => (400, Vec::new()),
            }
        })
        .await
    }

    #[tokio::test]
    async fn a_node_answers_what_it_let_through() {
        // The other node let "k" through twice, with nobody to ask.
        let other = Arc::new(RateLimiter::new(Some(Arc::new(Peers::for_test(vec![], 3)))));
        assert!(other.check("k", 10, MINUTE).await);
        assert!(other.check("k", 10, MINUTE).await);
        let limiter = RateLimiter::new(Some(Arc::new(Peers::for_test(
            vec![node(other.clone()).await],
            3,
        ))));
        // Two there and none here: one more goes, then none.
        assert!(limiter.check("k", 3, MINUTE).await);
        assert!(!limiter.check("k", 3, MINUTE).await);
        // The refusal was not counted there either.
        assert_eq!(
            other
                .answer(
                    serde_json::to_vec(&Asked {
                        window_secs: 60,
                        keys: vec![digest("k"), digest("other")],
                    })
                    .unwrap()
                    .as_slice()
                )
                .unwrap()
                .counts,
            vec![2, 0]
        );
        // Another key is free on both.
        assert!(limiter.check("other", 1, MINUTE).await);
        assert!(!limiter.check("other", 1, MINUTE).await);
    }

    #[test]
    fn a_question_is_checked() {
        let limiter = RateLimiter::new(None);
        assert!(limiter.answer(b"not a question").is_err());
        let too_many = Asked {
            window_secs: 60,
            keys: (0..=MAX_KEYS_ASKED).map(|i| i.to_string()).collect(),
        };
        assert!(
            limiter
                .answer(&serde_json::to_vec(&too_many).unwrap())
                .is_err()
        );
        // The limiter without peers authorizes nobody: a single node is
        // asked by no one.
        assert!(!limiter.authorizes(Some(SECRET.as_bytes())));
    }
}
