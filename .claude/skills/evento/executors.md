# Executors, migrations, and raw queries

`Executor` is the storage abstraction in `evento-core`. All write/read APIs in evento are generic over it.

```rust
#[async_trait]
pub trait Executor: Send + Sync + 'static {
    // Optional hooks (defaults shown):
    fn default_routing_key(&self) -> Option<&str> { None }   // stamped on unkeyed writes; inherited by subscriptions
    fn write_watch(&self) -> Option<watch::Receiver<u64>> { None } // in-process write signal for instant subscription wakeup
    fn stable_timestamp(&self) -> Option<u64> { None }       // replication stability watermark (µs); Accord only

    async fn write(&self, events: Vec<Event>) -> Result<(), WriteError>;
    async fn read(&self, aggregators: Option<Vec<EventFilter>>,
                  routing_key: Option<RoutingKey>, args: Args)
        -> anyhow::Result<ReadResult<Event>>;
    async fn latest_timestamp(&self, aggregators: Option<Vec<EventFilter>>,
                              routing_key: Option<RoutingKey>) -> anyhow::Result<u64>;
    async fn get_subscriber_cursor(&self, key: String) -> anyhow::Result<Option<Value>>;
    async fn is_subscriber_running(&self, key: String, worker_id: Ulid) -> anyhow::Result<bool>;
    async fn upsert_subscriber(&self, key: String, worker_id: Ulid) -> anyhow::Result<()>;
    async fn acknowledge(&self, key: String, cursor: Value, lag: u64) -> anyhow::Result<()>;
    async fn get_snapshot(&self, aggregate_type: String, aggregate_revision: String, id: String)
        -> anyhow::Result<Option<(Vec<u8>, Value)>>;
    async fn save_snapshot(&self, aggregate_type: String, aggregate_revision: String, id: String,
                           data: Vec<u8>, cursor: Value) -> anyhow::Result<()>;
    async fn delete_snapshot(&self, aggregate_type: String, id: String) -> anyhow::Result<()>;
}
```

You almost never implement this trait yourself — use one of the built-in executors:

| Type | Cargo feature | Crate |
|------|---------------|-------|
| `Sql<DB>` (`evento::Sqlite` / `MySql` / `Postgres`) | `sqlite` / `mysql` / `postgres` | `evento-sql` |
| `Fjall` | `fjall` | `evento-fjall` |
| `Evento` (type-erased `Arc<Box<dyn Executor>>`) | (always) | `evento-core` |
| `EventoGroup` | `group` | `evento-core` |
| `Rw<R, W>` | `rw` (implied by `fjall`) | `evento-core` |
| `Sidecar<E, L>` | `rw` | `evento-core` |
| `evento::RemoteClient` (`evento_remote::Client`) | `remote` | `evento-remote` |
| `AccordExecutor` (consensus-replicated) | — | `evento-accord` |

## SQL setup

```rust
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use evento::migrator::{Migrate, Plan};   // re-export of sqlx_migrator

let pool = SqlitePoolOptions::new()
    .max_connections(8)
    .connect_with(SqliteConnectOptions::new()
        .filename("events.db")
        .create_if_missing(true))
    .await?;

// 1) Run migrations once on startup (the DB type is REQUIRED on `new`)
let mut conn = pool.acquire().await?;
evento::sql_migrator::new::<sqlx::Sqlite>()?
    .run(&mut *conn, &Plan::apply_all())
    .await?;
drop(conn);

// 2) Wrap the pool — `From<Pool<DB>> for Sql<DB>` is provided
let executor: evento::Sqlite = pool.into();
```

For Postgres / MySQL, swap the generic parameter and pool type:

```rust
let executor: evento::Postgres = pg_pool.into();
let executor: evento::MySql   = my_pool.into();
```

`Sql<DB>` is `Clone` (cheap; clones the inner sqlx `Pool`, which is `Arc`-shared) — clones share the same in-process `write_watch` signal, so subscriptions wake instantly on writes made through any clone.

### Migrations

`evento::sql_migrator::new::<DB>()` returns a configured `sqlx_migrator::Migrator<DB>` (evento uses `sqlx_migrator` 0.19 / sqlx 0.9) with these migrations registered:

| Migration | Adds |
|-----------|------|
| `InitMigration` | `event`, `snapshot`, `subscriber` tables |
| `M0002` | `event.timestamp_subsec` column |
| `M0003` | Widens `event.name` |
| `M0004` | Replaces `idx_event_type` with a composite cursor-scan index |
| `M0005` | Leading-cursor index for no-routing-key subscription scans |

(The `accord` feature on `evento-sql-migrator` appends an `AccordMigration` for the consensus-journal tables.)

Tables after applying all:

**`event`** — `id VARCHAR(26)`, `name VARCHAR(50)`, `aggregator_type VARCHAR(50)`, `aggregator_id VARCHAR(26)`, `version INTEGER`, `data BLOB`, `metadata BLOB`, `routing_key VARCHAR(50)`, `timestamp BIGINT`, `timestamp_subsec BIGINT`.

**`subscriber`** — `key VARCHAR(50) PRIMARY KEY`, `worker_id VARCHAR(26)`, `cursor TEXT`, `lag INTEGER`, `enabled BOOLEAN`, `created_at TIMESTAMP`, `updated_at TIMESTAMP`.

> Note: the **physical column names are still `aggregator_type` / `aggregator_id`** — the alpha.21 rename to `aggregate_*` touched only the Rust API, so existing databases work unchanged.

Snapshots are stored via the executor's `get_snapshot` / `save_snapshot` / `delete_snapshot` methods — backends choose their own physical storage; the public `Snapshot` trait abstracts it.

## Fjall (embedded)

```rust
use evento::Fjall;    // re-export of evento_fjall::Fjall (feature "fjall")

let executor: Fjall = Fjall::open("./events")?;     // creates directories if missing
// or, custom config (fjall 3.x):
let db = fjall::Database::builder("./events").open()?;
let executor = Fjall::from_database(db)?;

executor.database();                                 // borrow the underlying fjall::Database
executor.persist()?;                                 // force fsync
```

No migrations — Fjall creates its partitions on first open. Data is laid out across seven partitions:

- `events` — primary storage: `ULID -> Event`
- `agg_index` — `{type}\0{id}\0{version}` → ULID (optimistic-concurrency check + per-instance scans)
- `agg_name_index` — `{type}\0{id}\0{name}\0{ULID}` → () (per-instance + event-name filters without deserializing)
- `routing_index` — `{routing_key}\0{ULID}` → ()
- `type_index` — `{type}\0{name}\0{ULID}` → ()
- `subscribers` — `{key}` → subscriber state
- `snapshots` — `{type}\0{id}` → snapshot record

Writes are serialized (single-writer lock), so concurrent appends can't both pass the optimistic version check, and multi-event batches for one aggregate validate in-batch.

## Type-erased / composite executors

### `Evento`

`evento::Evento::new(executor)` wraps any `Executor` in `Arc<Box<dyn Executor>>`. Use it when you want one type that can hold any backend (e.g., behind a trait object, in `axum::State`).

```rust
let any: evento::Evento = sqlite_executor.into();    // also: From<Sqlite> / From<&Sqlite> etc.

// Multi-tenancy: stamp unkeyed writes and scope subscriptions
let tenant = evento::Evento::new(sqlite_executor)
    .default_routing_key("tenant-a");
```

`default_routing_key` fills in missing routing keys on `write` (explicit per-aggregate keys win) and is inherited by `SubscriptionBuilder` / `ProjectionSubscription` when the user hasn't called `.routing_key()` or `.any_routing_key()`.

### `EventoGroup` (feature `group`)

Aggregates multiple `Evento` executors. Reads fan out to all executors and merge by cursor; writes (and subscriber/snapshot state) go to the first executor only. Used for read-only aggregation across multiple stores.

```rust
let group = evento::EventoGroup::default()
    .executor(primary)
    .executor(secondary);
```

### `Rw<R, W>` (feature `rw`)

Read/write split. Reads, `write_watch`, snapshot reads, and subscriber-cursor reads go to `R`; writes, snapshot saves/deletes, `upsert_subscriber`, and `acknowledge` go to `W`. Construct via `From<(R, W)>`:

```rust
let rw: evento::sql::RwSqlite = (read_pool.into(), write_pool.into()).into();
```

`RwSqlite` / `RwMySql` / `RwPostgres` are convenience aliases (`Rw<Sqlite, Sqlite>`, etc.).

### `Sidecar<E, L>` (feature `rw`, since alpha.35)

Events on one executor, a subscription's bookkeeping on another. Writes, reads,
`write_watch`, `stable_timestamp`, `latest_version` and `stream_routing_key` go to
`E` (typically a remote `Client` shared by many processes); `upsert_subscriber`,
`acknowledge`, worker fences and the snapshot calls go to `L`, a store this
process owns (a SQLite file, a Fjall directory). Construct via `new` or `From<(E, L)>`;
`events()` / `local()` give the halves back.

```rust
let sidecar: evento::Sidecar<evento::RemoteClient, evento::Sqlite> = (client, local_pool.into()).into();
```

- Use it for a replica of a client application that follows a shared store:
  replicas never fence each other out of one subscription key, nothing
  accumulates on the shared store, a restart resumes from the local cursor.
- `L` must belong to exactly one process; two processes sharing it share cursors again.
- Cursor keys stay `{E's routing key}.{name}`, so one local store may follow
  several event stores with distinct routing keys.
- `L` needs evento's tables: run `evento::sql_migrator` on the local pool too.

## Remote client/server (feature `remote`, `evento-remote`)

`serve(listener, executor)` exposes **any** executor over length-delimited framed
TCP; `Client` implements `Executor` by forwarding every call, so `evento::create`,
projections and subscriptions work unchanged against the remote store. Writes on
the server push a notification to every client, so client-side subscriptions wake
as fast as a local `write_watch`; the server's `stable_timestamp` piggybacks on
every frame.

```rust
// Server
let handle = evento_remote::serve(listener, executor);          // ServerHandle; dropping it does NOT stop the server
handle.shutdown().await;

// Client
let client = evento_remote::Client::connect("127.0.0.1:4321".parse()?).await?;
let client = evento_remote::Client::builder(addr)
    .token("tk_…")                                              // sent in the Hello frame
    .tls(connector, server_name)                                // rustls TlsConnector + ServerName
    .request_timeout(Duration::from_secs(10))
    .connections(2)                                             // pool size; clones share it
    .connect().await?;                                          // fails at once on a Rejection
```

Many stores and credentials (since alpha.34, wire `FORMAT_VERSION` 4): implement
`Resolver` and pass it to `serve_with` / `serve_tls_with`. Every connection opens
with `Hello { token: Option<Token>, peer }` (the token redacts its `Debug`); the
resolver answers once per connection (a reconnect is a new connection) with the
`Session` to serve it, or a `Rejection` the client surfaces as the error's source.

```rust
#[async_trait::async_trait]
impl evento_remote::Resolver for KeyResolver {
    type Executor = TenantExecutor;
    async fn resolve(&self, hello: &Hello) -> Result<Session<TenantExecutor>, Rejection> {
        let key = self.authenticate(hello.token.as_ref()).await?;   // load by hash, no secret compare
        let (executor, lease) = self.tenants.open(key.tenant_id).await?;
        Ok(Session::new(executor)
            .keep(lease)                       // anything to hold for the connection's lifetime
            .close_when(key.revoked_rx))       // watch::Receiver<bool>: true ends the connection
    }
}
let handle = evento_remote::serve_tls_with(listener, acceptor, resolver); // rustls TlsAcceptor
```

`serve` is the trivial resolver (tokens ignored). Plain TCP carries the token
in clear text: trusted networks only.

## Consensus cluster (`evento-accord`)

`AccordExecutor` replicates events across `N = 2f + 1` nodes (leaderless,
strictly serializable) over a `DataStore` + `Journal` the caller supplies
(`evento_sql::Sql` + `evento_sql::SqlJournal` on one SQLite file; the
`accord` feature of `evento-sql-migrator` adds the journal tables). One
process can host many groups: `GroupHost::new(mux, scheduler)` over one
`MuxTransport` (served once per process, `serve`/`serve_tls`/
`serve_tls_verified`) and one `SweepScheduler` (`start` once); `open(group,
topology, datastore, journal, config)` registers the inbox, recovers from the
journal and schedules sweeps (idempotent), `close(group)` unregisters it and
returns the `Node`. Frames for a closed group reach the transport's
`UnroutedHandler` (sweep gossip excluded since alpha.33), which is how a node
reopens a tenant a peer is talking about. A single node talks to itself over a
loopback port, so the same code serves both deployments. Reference:
`examples/bank-axum-accord-tenants` upstream; in this workspace
`dashboard/src/cluster.rs` and `dashboard/src/tenants.rs`.

## Raw paginated reads with `Reader` (SQL)

For custom read-models, use `evento::sql::Reader` — a thin wrapper over a sea-query `SelectStatement` that adds cursor-based pagination. (evento-sql builds on `sea-query` 1.0 / `sea-query-sqlx` 0.9 — see the `sea-query` skill.)

```rust
use evento::{
    sql::Reader,
    cursor::{Args, ReadResult},
};
use sea_query::{Expr, ExprTrait, Query};

// Build a sea-query statement against your own table
let stmt = Query::select()
    .columns([Account::Id, Account::Owner, Account::Balance, Account::CreatedAt])
    .from(Account::Table)
    .and_where(Expr::col(Account::Status).eq("active"))
    .to_owned();

let result: ReadResult<AccountRow> = Reader::new(stmt)
    .forward(20, None)                  // .forward(n, after_cursor) / .backward(n, before_cursor)
    .execute::<sqlx::Sqlite, AccountRow, _>(&pool)
    .await?;
```

Requirements on `AccountRow`:

- `sqlx::FromRow` for your DB.
- `evento::cursor::Cursor + evento::sql::Bind<Cursor = Self>` — usually generated by `#[derive(evento::Cursor)]`. See [`macros.md`](./macros.md) for the cursor derive.

`Reader` impls `Deref<Target = SelectStatement>` / `DerefMut`, so you can keep mutating the underlying query before `execute`:

```rust
let mut r = Reader::new(stmt);
r.and_where(Expr::col(Account::Currency).eq("USD"))
 .forward(20, after);
let page = r.execute::<sqlx::Sqlite, AccountRow, _>(&pool).await?;
```

`.desc()` / `.order(cursor::Order::Asc | Desc)` controls direction.

`evento::sql_types::Bitcode<T>` wraps bitcode-serialized values for BLOB columns (SQLite sqlx traits implemented; derefs to `T`).

## `EventFilter`

```rust
EventFilter::by_type("crate/BankAccount")                    // all events of a type
EventFilter::by_id("crate/BankAccount", account_id)          // one instance
EventFilter::by_event("crate/BankAccount", "MoneyDeposited") // one event name across instances
EventFilter::exact("crate/BankAccount", account_id, "MoneyDeposited") // full filter
```

Pass as `Some(vec![...])` to `Executor::read`. Multiple filters are OR'd; fields within one filter are AND'd. `None` means no aggregate filter.

## Snapshots

When `P: bitcode::Encode + bitcode::DecodeOwned + ProjectionCursor + Send + Sync`, evento auto-implements `Snapshot<E>` using the executor's `get_snapshot` / `save_snapshot` / `delete_snapshot`. A load:

1. Calls `P::restore(&context)` — usually returns the persisted snapshot.
2. Reads events forward from the snapshot cursor (or from the beginning).
3. Applies each event via the matching `Handler`.
4. If any events were applied, sets the cursor and calls `take_snapshot(&context)`.

To use custom snapshot storage (e.g., your own read-model table, a cache, or an in-memory `HashMap` for tests), implement `Snapshot<E>` for your projection type *manually* — that overrides the blanket impl. Override `drop_snapshot` too if you configured `Projection::tombstone::<Ev>()` and need the row deleted when the tombstone event arrives.

`Projection::revision(u16)` participates in the snapshot key. Bumping the revision invalidates all existing snapshots and forces a rebuild — use this when the projection's shape changes incompatibly. (Note: `save_snapshot` upserts on `(type, id)`, so only one snapshot row exists per aggregate.)
