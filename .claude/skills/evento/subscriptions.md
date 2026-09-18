# Subscriptions

`SubscriptionBuilder<E>` configures a long-running task that streams events from an executor, dispatches them to registered handlers, and acknowledges cursor progress per event.

## Builder API

```rust
use std::time::Duration;
use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder, Subscription},
};

let sub: Subscription = SubscriptionBuilder::<evento::Sqlite>::new("deposit-notifier")
    .handler(on_deposit())            // register handlers (one per event type)
    .handler(on_withdraw())
    .skip::<AccountClosed>()          // optional: explicit no-op
    .data(my_shared_state)            // optional: injected into handler ctx
    .routing_key("accounts")          // optional: only events with this key
    .chunk_size(100)                  // batch size; default 300
    .retry(5)                         // exponential backoff retries; default 30
    .delay(Duration::from_secs(10))   // delay before first poll
    .poll_interval(Duration::from_millis(250)) // fallback re-poll cadence; default 250ms
    .continue_on_error()              // continue past handler errors instead of stopping
    .strict()                         // fail if an event has no handler
    .start(&executor)                 // spawn background task
    .await?;

// later
sub.shutdown().await?;                // graceful: signals shutdown, awaits in-flight event
```

Alternative entry points:

- `.run_once(&executor)` — process all currently pending events once and return; does not spawn. (Takes `&mut self`, so bind the builder with `let mut`.)
- `.no_retry()` — disable retries; combine with `.start(..)` or `.run_once(..)`.

`Subscription { id: Ulid, … }`. The `id` is the worker id stored in the `subscriber` row.

`SubscriptionBuilder::aggregate::<A>(id)` scopes a handler's aggregate type to one specific instance id (mirrors `LoadBuilder::aggregate`).

## Routing keys

```rust
pub enum RoutingKey {
    All,                       // match all events regardless of key
    Value(Option<String>),     // match events whose routing_key equals this (None = unkeyed)
}
```

With no `.routing_key()` / `.all()` call, the subscription inherits the **executor's `default_routing_key()`** (set via `Evento::default_routing_key`); if the executor has none, it defaults to `RoutingKey::Value(None)` — *only events with no routing key*. To consume everything use `.all()`. To consume one partition, use `.routing_key("accounts")`.

Storage-key prefixing keeps cursors isolated:

- `.routing_key("accounts")` persists the cursor under `"accounts.my-sub"` instead of `"my-sub"` — two subscriptions with the same logical key but different routing keys keep separate cursors. This is the standard pattern for parallel partitioned consumers.
- `.all()` on an executor with a default routing key still prefixes the storage key with that default, so multi-tenant executors don't share one subscriber row.

## Handler dispatch

For each event in a chunk, the runtime computes two keys:

```
all_key = "{aggregate_type}_all"
key     = "{aggregate_type}_{event_name}"
```

It looks up `all_key` first, then `key`. Whichever matches first runs. So a `#[evento::subscription_all]` handler shadows per-event handlers on the same builder.

Multiple handlers for the same `(aggregate_type, event_name)` are rejected at registration: `SubscriptionBuilder::handler` panics with `"Cannot register event handler: key {…} already exists"`.

`strict()` flips an internal flag — when an event has no matching handler:

- disabled (default): event is skipped, cursor advances.
- enabled: `process` bails with `anyhow!("no handler …")`.

Read filters are derived from the registered handlers: with `strict()` off, each per-event handler reads only its own event name; `subscription_all` handlers always read by aggregate type.

## Error handling and retries

`SubscriptionBuilder::retry(n)` wraps each processing pass in `backon::ExponentialBuilder::default().with_max_times(n)`. If processing fails (an event handler returns `Err`):

- Each retry re-runs from the current cursor — only events that were not yet acknowledged are replayed.
- After all retries exhaust: `continue_on_error()` → log + continue on the next wake-up; otherwise the task exits.

Since already-acknowledged events are never replayed but a failed batch resumes from the last ack, handlers must be **idempotent** (key side effects on `event.id`, a ULID).

## Wake-up, polling, and acknowledgement

The worker loop wakes on whichever comes first:

1. **Write signal** — `Executor::write_watch()` returns a `tokio::sync::watch::Receiver<u64>` bumped after each successful in-process write (all built-in backends support it). Events committed through the same executor instance (or a clone) are processed with near-zero latency.
2. **Fallback poll tick** — `poll_interval` (default 250ms) bounds latency for writes the signal can't observe (other processes, custom executors).
3. **Shutdown signal.**

Each processing pass then drains all available events back-to-back: read `chunk_size` events forward from the cursor, run the handler for each, and `executor.acknowledge(key, event.cursor, lag)` after every event (`lag` = latest matching event timestamp minus this event's timestamp, saturating at 0). A full chunk triggers an immediate next read; a partial chunk means it's caught up.

**Replicated backends:** `Executor::stable_timestamp()` (the Accord backend) returns a stability watermark; the subscription holds back events at or above it so a late lower-cursor event can't be skipped. Single-store backends return `None` (no gating).

## Context API

```rust
pub struct Context<'a, E: Executor> {
    pub executor: &'a E,
    /* deref target: RwContext */
}
```

Inside a handler:

```rust
use evento::{EventFilter, cursor::Args};

#[evento::subscription]
async fn on_deposit<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<MoneyDeposited>,
) -> anyhow::Result<()> {
    // 1) executor for downstream queries
    let history = ctx.executor.read(
        Some(vec![EventFilter::by_id("crate/BankAccount", &event.aggregate_id)]),
        None,
        Args::forward(50, None),
    ).await?;

    // 2) shared data (must be Clone for RwContext::extract)
    let config: AppConfig = ctx.extract();        // panics if not injected

    // 3) optional non-panicking accessor:
    if let Some(cfg) = ctx.get::<AppConfig>() { /* … */ }

    Ok(())
}
```

Inject data with `.data(value)` on the builder. Values are keyed by `TypeId` — only one instance per type at a time. Wrap in `evento::context::Data<T>` (an `Arc`) when you want cheap clones.

`Context` derefs to `evento::context::RwContext`, an `Arc<RwLock<…>>` over a `HashMap<TypeId, Box<dyn Any + Send + Sync>>`.

## Graceful shutdown

```rust
let sub = builder.start(&executor).await?;
// … running …
sub.shutdown().await?;            // sends signal, awaits join handle
```

`shutdown` sends on a `oneshot` channel; the worker selects on it between wake-ups and also checks it between events inside a batch, then breaks the loop. The error type is `tokio::task::JoinError` (panicked or cancelled).

If `continue_on_error` is *off* and a handler errors past the retry budget, the task exits silently — `sub.shutdown().await?` still returns `Ok(())` because the join handle completes normally. Use tracing to surface those failures (the worker emits `tracing` spans per event: `subscription`, `aggregate_type`, `aggregate_id`, `event`).

## Cursor recovery

Subscription state lives in the `subscriber` table (SQL) or `subscribers` partition (Fjall):

```
{ key, worker_id, cursor, lag, enabled, created_at, updated_at }
```

`SubscriptionBuilder::start` calls `executor.upsert_subscriber(key, new_worker_id)` — meaning **only one worker per `key` can be running**. If a second worker calls `start` with the same key, the previous worker's next `is_subscriber_running` check returns `false` and that task exits. Use distinct keys (or distinct routing keys, which prefix the key) for parallel workers.

To pause a subscription externally, set `enabled = false` on its row — the worker stops on the next iteration.

## Common patterns

**Auto-updated projection.** Prefer `Projection::new::<A>().handler(..).subscription("key").start(&exec)` over a hand-rolled read-model subscription — it reloads the affected aggregate and persists the snapshot for you (see `SKILL.md`, including the co-keying caveat for secondary aggregates).

**Read-model writer (custom table).** Subscription with `.routing_key(...)` per partition, handlers that upsert into a SQL table keyed by `event.aggregate_id`. Combine with `evento::append(id).original_version(v)` for cross-aggregate event emission.

**Side-effects (email/webhook).** Subscription with `.retry(n).continue_on_error()` so a flaky external API doesn't kill the worker. Make handlers idempotent by keying on `event.id` (a ULID).

**One-shot catch-up.** `SubscriptionBuilder::new(key).handler(…).run_once(&executor)` processes everything from the cursor to head and returns — useful for migrations / backfills and tests.

**Audit log.** `#[evento::subscription_all]` recording `event.name`, `event.aggregate_type`, `event.aggregate_id`, `event.metadata.requested_by()` into a write-only table.
