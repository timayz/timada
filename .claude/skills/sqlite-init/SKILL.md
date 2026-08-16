---
name: sqlite-init
description: Reference for initializing SQLite connection pools with `sqlx` in Rust. Use whenever writing, editing, or reviewing code that creates `SqlitePool`s, configures SQLite pragmas, sets `journal_mode` / `synchronous` / `busy_timeout` / `foreign_keys` / `wal_autocheckpoint` / `journal_size_limit` / `mmap_size`, or splits read vs write pools. Covers the read-only pool (sized to CPU cores, no WAL), the single-connection write pool (WAL + NORMAL synchronous, serializes writes to avoid `SQLITE_BUSY`, `wal_autocheckpoint = 0` so Litestream owns checkpointing), and the standard single-pool setup for CLI/test contexts (long 60s busy_timeout for WAL recovery on slow storage). Also flags common anti-patterns: setting `journal_mode = WAL` on a read-only pool, omitting `busy_timeout`, missing `BEGIN IMMEDIATE` for write transactions, applying pragmas in app code instead of `connect_with` (which loses them on replacement connections), and app-initiated WAL checkpoints while Litestream is replicating.
---

# SQLite — Connection Pool Initialization

This codebase uses [`sqlx`](https://docs.rs/sqlx) with SQLite. All pools MUST be created via the helpers in this file's `base_options` pattern so per-connection pragmas survive idle-replacement of connections.

Source: <https://github.com/timayz/imkitchen/blob/main/src/db.rs>

## The three pools

| Pool                 | Purpose                                          | Max connections   | `journal_mode` | `synchronous` | `busy_timeout` | `wal_autocheckpoint` |
| -------------------- | ------------------------------------------------ | ----------------- | -------------- | ------------- | -------------- | -------------------- |
| `create_read_pool`   | Concurrent reads from web handlers / queries     | CPU cores         | (not set)      | (not set)     | 5s             | 1000 (base)          |
| `create_write_pool`  | Serialized writes — all `BEGIN IMMEDIATE` txns   | **1**             | `WAL`          | `NORMAL`      | 5s             | **0** (Litestream)   |
| `create_pool`        | CLI tools (migrate, import, tests) — single pool | caller-specified  | `WAL`          | `NORMAL`      | **60s**        | 1000 (base)          |

Pick the pair (`read_pool` + `write_pool`) for the long-running server. Use the single `create_pool` for short-lived CLI commands and tests.

## Reference implementation

```rust
use anyhow::Result;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use std::str::FromStr;
use std::time::Duration;

/// Base connect options shared by every pool.
///
/// Returns options with all per-connection pragmas configured. sqlx re-applies
/// these every time it opens a new connection, including replacement connections
/// after idle timeout — which is the behavior you want.
fn base_options(database_url: &str, busy_timeout: Duration) -> Result<SqliteConnectOptions> {
    Ok(
        SqliteConnectOptions::from_str(database_url)?
            .busy_timeout(busy_timeout)
            .foreign_keys(true)
            .pragma("wal_autocheckpoint", "1000") // explicit; write pool overrides to 0 (Litestream owns checkpointing)
            .pragma("journal_size_limit", "67108864")
            .pragma("cache_size", "-20000")
            .pragma("mmap_size", "268435456") // 256 MiB memory-mapped I/O — cuts read syscalls
            .pragma("temp_store", "memory"),
    )
}

/// Read-only pool, optimized for concurrent reads.
///
/// Sized to CPU cores. Does NOT set `journal_mode` or `synchronous` — those are
/// write-side concerns and `PRAGMA journal_mode = WAL` would fail on a read-only
/// connection anyway. The DB file's journal mode is set by the write pool.
pub async fn create_read_pool(database_url: &str, max_connections: u32) -> Result<SqlitePool> {
    let options = base_options(database_url, Duration::from_millis(5000))?.read_only(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect_with(options)
        .await?;

    tracing::info!(
        "Created read-only pool with {} max connections",
        max_connections
    );
    Ok(pool)
}

/// Read-write pool, single connection to serialize writes and avoid SQLITE_BUSY.
///
/// All write transactions go through this pool. Use `BEGIN IMMEDIATE` for any
/// transaction that will write, so it grabs the reserved lock up front instead
/// of upgrading mid-transaction (which is what causes most BUSY errors).
pub async fn create_write_pool(database_url: &str) -> Result<SqlitePool> {
    let options = base_options(database_url, Duration::from_millis(5000))?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        // Disable SQLite's automatic WAL checkpoint at runtime. When Litestream is
        // replicating, it must be the sole owner of checkpointing — an app-initiated
        // checkpoint can discard WAL frames Litestream hasn't shipped yet and break the
        // replication chain. journal_size_limit still bounds -wal growth as a backstop.
        // (migrate's create_pool keeps autocheckpoint + explicit TRUNCATE — it runs before
        // the Litestream sidecar starts.)
        .pragma("wal_autocheckpoint", "0");

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;

    tracing::info!("Created read-write pool with 1 max connection");
    Ok(pool)
}

/// Standard pool for CLI commands (migrate, import, tests).
///
/// Single-pool setup, so it owns the WAL/synchronous settings.
///
/// Uses a long `busy_timeout` because migrate/reset can open a database with a large
/// leftover `-wal` from a previous unclean shutdown; recovering/checkpointing it on slow
/// network storage (e.g. Longhorn) can take well over the 5s the serve pools use.
pub async fn create_pool(database_url: &str, max_connections: u32) -> Result<SqlitePool> {
    let options = base_options(database_url, Duration::from_secs(60))?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal);

    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect_with(options)
        .await?;

    tracing::info!("Created pool with {} max connections", max_connections);
    Ok(pool)
}
```

## Why this shape

### `base_options` returns a builder, not a connection

`SqliteConnectOptions` is **per-connection** state. `sqlx` re-applies it every time the pool opens a new connection — including replacement connections created after `idle_timeout` evicts an old one. If you set pragmas via `pool.execute("PRAGMA ...")` after the pool is built, those pragmas are lost on the *next* connection sqlx opens. Always set pragmas inside `base_options` so they survive connection replacement.

### Read pool: no `journal_mode`, no `synchronous`

Both are **write-side** concerns. A read-only connection cannot execute `PRAGMA journal_mode = WAL` — it would fail because changing journal mode requires writing to the database header. The DB file's journal mode is owned by the write pool (or by whichever process first opens the file writable).

`read_only(true)` is what makes this pool safe to size up: many concurrent readers, zero contention for the writer lock.

### Write pool: `max_connections = 1`

SQLite serializes writers at the file level. A pool of 5 writers does not get you 5× throughput — it gets you a pile of `SQLITE_BUSY` errors as connections fight for the reserved lock. With `max_connections = 1`, sqlx queues writes inside the pool (where you have backpressure and fairness) instead of pushing them into SQLite (where you get busy-loops).

### `WAL` + `synchronous = NORMAL`

- `WAL` lets readers keep reading while a writer is active. Without it, every write blocks every reader.
- `synchronous = NORMAL` is the recommended pairing with WAL — `FULL` is overkill (it fsyncs after every transaction; WAL already syncs at checkpoints) and `OFF` risks DB corruption on crash. `NORMAL` is durable across application crashes and durable enough across OS crashes for almost every workload.

### `busy_timeout` — a parameter: 5s for serve pools, 60s for CLI

If a write does collide (e.g. another process), SQLite will retry for up to the timeout before returning `SQLITE_BUSY`. Without this, you get immediate failures.

- **Serve pools (read + write): 5s** — long enough to absorb checkpoint stalls, short enough that a true deadlock surfaces.
- **CLI `create_pool`: 60s** — migrate/reset can open a database with a large leftover `-wal` from a previous unclean shutdown; recovering/checkpointing it on slow network storage (e.g. Longhorn) can take well over 5s.

### `wal_autocheckpoint` — 1000 in base, **0 on the write pool** (Litestream)

The base sets the SQLite default (`1000` pages) explicitly. The write pool overrides it to `0`, disabling SQLite's automatic WAL checkpointing at runtime: when Litestream is replicating, it must be the **sole owner of checkpointing** — an app-initiated checkpoint can discard WAL frames Litestream hasn't shipped yet and break the replication chain. `journal_size_limit` still bounds `-wal` growth as a backstop. The CLI `create_pool` keeps autocheckpointing (plus an explicit `TRUNCATE` checkpoint in migrate) because it runs *before* the Litestream sidecar starts.

### `journal_size_limit = 67108864`

Caps the `-wal` file at 64 MiB: after a checkpoint, the WAL is truncated back down instead of being left at its high-water mark. This is the backstop that keeps the WAL bounded on the write pool where autocheckpointing is off.

### `mmap_size = 268435456`

256 MiB of memory-mapped I/O — page reads come from the OS page cache via `mmap` instead of `read()` syscalls, cutting per-query syscall overhead on read-heavy workloads. The memory is file-backed and shared, not per-connection heap.

### `cache_size = -20000`

Negative means "kilobytes" (positive means "pages"). `-20000` = 20 MB of page cache per connection. Tune up for read-heavy workloads with hot indexes, down on memory-constrained hosts.

### `foreign_keys = true`

SQLite ships with FK enforcement **off** by default for historical reasons. Always turn it on. Without it, `ON DELETE CASCADE` and FK constraints are silently ignored.

### `temp_store = memory`

Keeps temp B-trees (used by `ORDER BY` without an index, `DISTINCT`, large `GROUP BY`) in RAM rather than spilling to a temp file.

## Writing transactions correctly

Always use `BEGIN IMMEDIATE` for any transaction that will write, even if the first statement is a `SELECT`:

```rust
let mut tx = write_pool.begin().await?;
sqlx::query("BEGIN IMMEDIATE").execute(&mut *tx).await?; // grab reserved lock now
// ... reads and writes ...
tx.commit().await?;
```

Why: SQLite's default `BEGIN` is `DEFERRED`. It starts as a reader and tries to upgrade to a writer on the first `INSERT`/`UPDATE`/`DELETE`. If another writer slipped in between, the upgrade fails with `SQLITE_BUSY` — and the busy-timeout retry **does not help here** (it only helps when acquiring a lock, not when upgrading from shared to reserved). `BEGIN IMMEDIATE` grabs the reserved lock up front; the busy-timeout then covers the wait.

## Anti-patterns to flag

- **Setting pragmas via `pool.execute("PRAGMA ...")`** — lost on replacement connections. Move them into `SqliteConnectOptions` via `.pragma(...)` or the typed builders.
- **`journal_mode(Wal)` on the read pool** — will error on connect, or silently no-op depending on sqlx version. Read-only pools must not set `journal_mode`.
- **Write pool with `max_connections > 1`** — produces `SQLITE_BUSY` under load. Always 1.
- **Plain `begin()` for write transactions** — defaults to `DEFERRED`. Upgrade-time `BUSY` is not retried by `busy_timeout`.
- **Missing `foreign_keys(true)`** — silently disables FK constraints. Always include.
- **No `busy_timeout`** — immediate `BUSY` errors instead of waiting through transient contention.
- **`synchronous = OFF`** — DB corruption risk on crash. Use `NORMAL` with WAL.
- **`synchronous = FULL` with WAL** — performance regression for no durability gain in typical workloads.
- **Setting `journal_mode = WAL` on every connect after the first** — it's a persistent setting on the DB file; setting it once is enough. sqlx will re-issue it harmlessly, but app code should not.
- **App-initiated WAL checkpoints (`PRAGMA wal_checkpoint`, or leaving `wal_autocheckpoint` on) while Litestream replicates** — a checkpoint can discard WAL frames Litestream hasn't shipped yet and break the replication chain. The runtime write pool sets `wal_autocheckpoint = 0`; only pre-sidecar tooling (migrate) may checkpoint.
- **Reusing the serve pools' 5s `busy_timeout` for migrations** — WAL recovery of a large leftover `-wal` on slow storage can exceed it and fail the migration spuriously. Use the 60s CLI pool.

## Tracing

Pool creation logs at `info!`. Per-statement logging is **disabled** in the reference implementation — to see queries during local development, temporarily add `.log_statements(tracing::log::LevelFilter::Debug)` to `base_options` (needs `use sqlx::ConnectOptions;`) and run with `RUST_LOG` / `EnvFilter` at `debug` for the relevant module. Don't ship it enabled: statement logging is noisy and can leak bound values into logs.

See the `tracing-logging` skill for the broader logging conventions.
