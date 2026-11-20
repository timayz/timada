//! The subscriptions this node runs, kept running.
//!
//! A subscription's worker ends for good on its own once a pass kept failing
//! past its retries: the store was away longer than that, or a handler
//! cannot deal with one event. Left there, what it feeds goes stale on this
//! node for every tenant, silently. So every worker is started supervised
//! (evento's `SubscriptionBuilder::supervised`): it goes on from its saved
//! cursor after a growing delay (at the event that failed: a cursor is
//! acknowledged only past the handlers that ran), one second doubling up to
//! five minutes while it fails again soon after, the wait and the count
//! forgotten once it went on that long without failing. How each stands is
//! read from here for `/admin/cluster`, the metrics and `/ready`.

use std::sync::Arc;

use anyhow::{Context as _, Result};
use evento::subscription::{Lag, State, Subscription, SubscriptionBuilder};

use crate::cluster::Executor;

/// How a worker stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerState {
    Running,
    /// A pass failed and the worker goes on from its cursor after a delay.
    /// `since` is when it first failed (unix seconds) and `attempts` how
    /// many restarts were tried since, the one being waited for included; a
    /// restart that failed again soon after counts on.
    Restarting {
        error: String,
        since: u64,
        attempts: u32,
    },
    /// Ended and not started again: on shutdown, or because a handler asked
    /// or another worker took the subscription over.
    Stopped {
        reason: String,
    },
}

impl WorkerState {
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }
}

/// A supervised subscription.
pub struct Worker {
    name: String,
    /// The key its cursor is kept under: its own behind the node's routing
    /// key, when it has one.
    key: Option<String>,
    handle: Subscription,
}

impl Worker {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The key the subscription's cursor is kept under, and how far the
    /// subscription is behind the newest event it follows, now. Asked of
    /// the store, with the subscription's own filters: it says the truth
    /// whether the worker runs, waits to go on, stopped, or sits in a
    /// handler that does not return, and an event the subscription does not
    /// follow never counts.
    pub async fn lag(&self) -> Result<(Option<String>, Lag)> {
        let lag = self.handle.lag().await.with_context(|| {
            format!(
                "cannot tell how far behind the {} subscription is",
                self.name
            )
        })?;
        Ok((self.key.clone(), lag))
    }

    pub fn state(&self) -> WorkerState {
        match self.handle.state() {
            State::Running => WorkerState::Running,
            State::Restarting {
                error,
                since,
                attempts,
            } => WorkerState::Restarting {
                error: format!("{error:#}"),
                since,
                attempts,
            },
            State::Stopped(reason) => WorkerState::Stopped {
                reason: reason.to_string(),
            },
            // A state evento names later: shown as it says it.
            other => WorkerState::Stopped {
                reason: other.to_string(),
            },
        }
    }

    /// Signals the worker to stop, without waiting for it.
    pub fn stop(&self) {
        self.handle.stop();
    }

    /// Waits for the worker to be done with the event in hand and stop;
    /// [`stop`](Self::stop) first.
    pub async fn stopped(&self) {
        self.handle.stopped().await;
    }
}

/// Starts the subscription `builder` supervised, under the name `name`. The
/// first start failing is an error: the node does not serve without its
/// workers. Later failures are gone on from.
pub async fn start(
    name: impl Into<String>,
    builder: SubscriptionBuilder<Executor>,
    executor: &Executor,
) -> Result<Arc<Worker>> {
    let name = name.into();
    let mut builder = builder.supervised();
    let key = builder.storage_key(executor);
    let handle = builder
        .start(executor)
        .await
        .with_context(|| format!("cannot start the {name} subscription"))?;
    Ok(Arc::new(Worker { name, key, handle }))
}

/// Runs the subscription `builder` over everything the store holds for it,
/// once, before anything is served: after the store was copied from a peer,
/// the read models are empty until their subscriptions caught up.
pub async fn catch_up(
    name: &str,
    builder: SubscriptionBuilder<Executor>,
    executor: &Executor,
) -> Result<()> {
    // One more try at a failing pass, no more: a store that stays away or an
    // event a handler cannot deal with is for the supervised worker, which
    // resumes at the cursor the pass left.
    builder
        .retry(1)
        .run_once(executor)
        .await
        .with_context(|| format!("the {name} subscription cannot catch up"))
}

/// Every worker of the node, for the cluster page.
#[derive(Clone, Default)]
pub struct Workers(Arc<Vec<Arc<Worker>>>);

impl Workers {
    pub fn new(workers: Vec<Arc<Worker>>) -> Self {
        Self(Arc::new(workers))
    }

    /// Each worker's name and how it stands, in start order.
    pub fn states(&self) -> Vec<(String, WorkerState)> {
        self.0
            .iter()
            .map(|worker| (worker.name().to_owned(), worker.state()))
            .collect()
    }

    /// Each worker's cursor key and how far behind it is now
    /// ([`Worker::lag`]), in start order.
    pub async fn lags(&self) -> Result<Vec<(Option<String>, Lag)>> {
        let mut lags = Vec::with_capacity(self.0.len());
        for worker in self.0.iter() {
            lags.push(worker.lag().await?);
        }
        Ok(lags)
    }

    /// Stops every worker and waits for them.
    pub async fn shutdown(&self) {
        for worker in self.0.iter() {
            worker.stop();
        }
        for worker in self.0.iter() {
            worker.stopped().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use evento::metadata::Event;
    use evento::subscription::Context;
    use evento::{Evento, Turso};
    use timada_tenancy::aggregator::Created;
    use timada_tenancy::{Command, CreateTenant};

    use super::*;

    /// What the handler saw: the tenant names, in order, failures counted
    /// apart.
    #[derive(Clone, Default)]
    struct Seen {
        names: Arc<Mutex<Vec<String>>>,
        failures_left: Arc<AtomicUsize>,
    }

    #[evento::subscription]
    async fn on_created<E: evento::Executor>(
        context: &Context<'_, E>,
        event: Event<Created>,
    ) -> anyhow::Result<()> {
        let seen: Seen = context.extract();
        if event.data.name == "poison"
            && seen
                .failures_left
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                    left.checked_sub(1)
                })
                .is_ok()
        {
            anyhow::bail!("cannot deal with {}", event.data.name);
        }
        seen.names.lock().unwrap().push(event.data.name.clone());
        Ok(())
    }

    fn builder(seen: Seen) -> SubscriptionBuilder<Executor> {
        SubscriptionBuilder::new("test/created")
            .data(seen)
            .handler(on_created())
            .any_routing_key()
            .retry(0)
    }

    async fn create(store: &Evento, name: &str) {
        Command(store.clone())
            .create_tenant(CreateTenant {
                name: name.into(),
                owner_user_id: "alice".into(),
            })
            .await
            .unwrap();
    }

    async fn eventually(what: &str, mut ok: impl AsyncFnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !ok().await {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{what}"));
    }

    /// A pass that fails past its retries ends the worker; the supervisor
    /// starts it again from the event that failed, so nothing is skipped and
    /// nothing is replayed, and the page sees it restarting then running.
    #[tokio::test]
    async fn a_failed_worker_is_restarted_at_the_event_that_failed() {
        let store = Evento::new(Turso::in_memory_concurrent().await.unwrap());
        let seen = Seen::default();
        seen.failures_left.store(1, Ordering::SeqCst);
        create(&store, "first").await;
        let worker = start("created", builder(seen.clone()), &store)
            .await
            .unwrap();
        eventually("the first event is seen", async || {
            seen.names.lock().unwrap().len() == 1
        })
        .await;
        assert_eq!(worker.state(), WorkerState::Running);

        create(&store, "poison").await;
        create(&store, "last").await;
        eventually("the worker is seen restarting", async || {
            matches!(worker.state(), WorkerState::Restarting { attempts: 1, .. })
        })
        .await;
        eventually(
            "the worker runs again and gets past the event",
            async || seen.names.lock().unwrap().len() == 3,
        )
        .await;
        assert_eq!(worker.state(), WorkerState::Running);
        assert_eq!(
            *seen.names.lock().unwrap(),
            ["first", "poison", "last"],
            "resumed at the failing event, which passed on the second delivery"
        );

        worker.stop();
        worker.stopped().await;
        assert_eq!(
            worker.state(),
            WorkerState::Stopped {
                reason: "shutdown requested".to_owned()
            }
        );
    }

    /// A worker that fails again right after its restart is waited for
    /// longer each time, and the page sees one run of failures, not a new
    /// one every half minute.
    #[tokio::test]
    async fn a_worker_failing_again_at_once_waits_longer_and_counts_on() {
        let store = Evento::new(Turso::in_memory_concurrent().await.unwrap());
        let seen = Seen::default();
        seen.failures_left.store(2, Ordering::SeqCst);
        let worker = start("created", builder(seen.clone()), &store)
            .await
            .unwrap();
        create(&store, "poison").await;
        let first = tokio::time::Instant::now();
        eventually("the first failure is seen", async || {
            matches!(worker.state(), WorkerState::Restarting { attempts: 1, .. })
        })
        .await;
        let WorkerState::Restarting { since, .. } = worker.state() else {
            unreachable!()
        };
        // Restarted after 1 s, fails again at once: 2 s before the next.
        eventually("the second failure counts on", async || {
            matches!(worker.state(), WorkerState::Restarting { attempts: 2, .. })
        })
        .await;
        assert!(matches!(worker.state(), WorkerState::Restarting { since: s, .. } if s == since));
        eventually("the third start gets past the event", async || {
            seen.names.lock().unwrap().len() == 1
        })
        .await;
        assert!(
            first.elapsed() >= Duration::from_secs(3),
            "1 s then 2 s of waiting, not 1 s twice"
        );
        assert_eq!(worker.state(), WorkerState::Running);
        worker.stop();
        worker.stopped().await;
    }

    /// How far behind a worker is, is asked of the store with what the
    /// subscription follows (#713): an event of another kind, however new,
    /// leaves it caught up, and the event its handler fails on keeps it
    /// behind while the worker waits to be started again, until it got past
    /// it.
    #[tokio::test]
    async fn a_worker_is_behind_by_the_events_it_follows_alone() {
        use evento::Executor as _;

        let store = Evento::new(Turso::in_memory_concurrent().await.unwrap());
        let seen = Seen::default();
        seen.failures_left.store(usize::MAX, Ordering::SeqCst);
        let worker = start("created", builder(seen.clone()), &store)
            .await
            .unwrap();
        let (key, lag) = worker.lag().await.unwrap();
        assert_eq!(key.as_deref(), Some("test/created"));
        assert!(lag.is_caught_up());

        // Nothing the subscription follows: its cursor does not move, and
        // it is not behind.
        store
            .write(vec![evento::Event {
                id: ulid::Ulid::generate(),
                aggregate_type: "test/Other".into(),
                aggregate_id: "other".into(),
                version: 1,
                name: "Happened".into(),
                ..Default::default()
            }])
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(worker.lag().await.unwrap().1.is_caught_up());

        create(&store, "poison").await;
        eventually("the worker is seen restarting", async || {
            matches!(worker.state(), WorkerState::Restarting { .. })
        })
        .await;
        let behind = worker.lag().await.unwrap().1;
        assert!(behind.pending_since.is_some(), "{behind:?}");

        seen.failures_left.store(0, Ordering::SeqCst);
        eventually("the worker gets past the event", async || {
            worker.lag().await.unwrap().1.is_caught_up()
        })
        .await;
        assert_eq!(*seen.names.lock().unwrap(), ["poison"]);
        worker.stop();
        worker.stopped().await;
    }

    /// A shutdown during the restart wait ends the supervisor at once.
    #[tokio::test]
    async fn a_shutdown_while_waiting_to_restart_is_prompt() {
        let store = Evento::new(Turso::in_memory_concurrent().await.unwrap());
        let seen = Seen::default();
        seen.failures_left.store(usize::MAX, Ordering::SeqCst);
        let workers = Workers::new(vec![
            start("created", builder(seen.clone()), &store)
                .await
                .unwrap(),
        ]);
        create(&store, "poison").await;
        eventually("the worker is seen restarting", async || {
            !workers.states()[0].1.is_running()
        })
        .await;
        tokio::time::timeout(Duration::from_secs(5), workers.shutdown())
            .await
            .expect("the shutdown does not wait out the restart delay");
        assert!(matches!(workers.states()[0].1, WorkerState::Stopped { .. }));
    }
}
