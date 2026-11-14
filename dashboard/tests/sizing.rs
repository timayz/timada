//! What a node's store costs on disk and per write (#477): bytes per event,
//! how far the commit log and the write-ahead log grow between checkpoints,
//! how long a write waits when one runs, and what the subscriptions woken by
//! every write add. A measurement, not a check: it is ignored by default and
//! prints its numbers.
//!
//! ```text
//! cargo test -p timada-dashboard --test sizing -- --ignored --nocapture
//! ```
//!
//! `SIZING_COMMANDS` (100000) catalog commands, about two events each, are
//! run by `SIZING_WRITERS` (20) writers at once on one node over a file store opened as `serve` opens it,
//! with `SIZING_SUBSCRIPTIONS` (0) subscriptions that follow none of those
//! events, as most of a node's sagas do for most writes. `SIZING_DIR` keeps
//! the store where it says instead of a temporary directory, on the disk to
//! measure.
//!
//! The dashboard is a binary crate, so the runtime is reached through
//! `#[path]` includes, which drag in items this test never calls.
#![allow(dead_code)]

#[path = "../src/cluster.rs"]
mod cluster;
#[path = "../src/config.rs"]
mod config;
#[path = "../src/db.rs"]
mod db;
#[path = "../src/mailroom/schema.rs"]
mod mailroom;
#[path = "../src/media/schema.rs"]
mod media;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use evento::metadata::Event;
use evento::subscription::{Context, SubscriptionBuilder};
use timada_catalog::CreateProduct;
use timada_tenancy::aggregator::Created;
use timada_tenancy::{Command, CreateTenant};

use cluster::Executor;

/// Commands one product gets: its creation, then renames. Each writes two
/// events: the product's own and its slug's claim.
const COMMANDS_PER_PRODUCT: u64 = 10;

fn knob(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .map(|v| v.parse().unwrap_or_else(|_| panic!("{name} is a number")))
        .unwrap_or(default)
}

/// The sizes of the store's file, write-ahead log and commit log.
fn sizes(path: &Path) -> [u64; 3] {
    db::store_files(path).map(|file| std::fs::metadata(file).map_or(0, |m| m.len()))
}

/// The processor time this process used so far, user and system.
fn cpu_time() -> Duration {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
    // The fields after the command name, which may hold spaces.
    let fields: Vec<&str> = stat[stat.rfind(')').unwrap() + 2..].split(' ').collect();
    let ticks: u64 = fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap();
    // Linux counts in hundredths of a second (`USER_HZ`).
    Duration::from_millis(ticks * 10)
}

async fn number(path: &Path, sql: &str) -> f64 {
    let db = evento::turso::turso::Builder::new_local(path.to_str().unwrap())
        .build()
        .await
        .unwrap();
    let conn = db.connect().unwrap();
    let mut rows = conn.query(sql, ()).await.unwrap();
    rows.next().await.unwrap().unwrap().get::<f64>(0).unwrap()
}

#[evento::subscription]
async fn on_tenant_created<E: evento::Executor>(
    _context: &Context<'_, E>,
    _event: Event<Created>,
) -> anyhow::Result<()> {
    Ok(())
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "a measurement: run it alone, with --nocapture"]
async fn what_a_store_costs() {
    let commands = knob("SIZING_COMMANDS", 100_000);
    let writers = knob("SIZING_WRITERS", 20);
    let subscriptions = knob("SIZING_SUBSCRIPTIONS", 0);
    let dir = std::env::var("SIZING_DIR").map_or_else(
        |_| std::env::temp_dir().join(format!("timada-sizing-{}", ulid::Ulid::generate())),
        PathBuf::from,
    );
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("timada.db");
    assert!(!path.exists(), "{} holds a store already", path.display());

    let store = db::open_store(&path).await.unwrap();
    let (cluster, executor, _) = cluster::open(None, None, &[], false, &path, store)
        .await
        .unwrap();
    let tenant = Command(executor.clone())
        .create_tenant(CreateTenant {
            name: "sizing".into(),
            owner_user_id: "alice".into(),
        })
        .await
        .unwrap();
    let mut workers = Vec::new();
    for n in 0..subscriptions {
        workers.push(
            SubscriptionBuilder::<Executor>::new(format!("sizing/{n}"))
                .handler(on_tenant_created())
                .start(&executor)
                .await
                .unwrap(),
        );
    }
    let empty = sizes(&path);

    // The logs' sizes, looked at ten times a second: their peaks.
    let peaks = Arc::new([AtomicU64::new(0), AtomicU64::new(0)]);
    let sampler = tokio::spawn({
        let (path, peaks) = (path.clone(), peaks.clone());
        async move {
            loop {
                let [_, wal, log] = sizes(&path);
                peaks[0].fetch_max(wal, Ordering::Relaxed);
                peaks[1].fetch_max(log, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    });

    let products = commands / COMMANDS_PER_PRODUCT / writers;
    let cpu = cpu_time();
    let started = Instant::now();
    let mut tasks = tokio::task::JoinSet::new();
    for w in 0..writers {
        let catalog = timada_catalog::Command::new(executor.clone(), &tenant.id);
        tasks.spawn(async move {
            // How long each command took, in microseconds.
            let mut took = Vec::new();
            for p in 0..products {
                let at = Instant::now();
                let product = catalog
                    .create_product(
                        "alice",
                        CreateProduct {
                            name: format!("Thing {w}.{p}"),
                            slug: None,
                        },
                    )
                    .await
                    .unwrap();
                took.push(at.elapsed().as_micros() as u64);
                for r in 1..COMMANDS_PER_PRODUCT {
                    let at = Instant::now();
                    catalog
                        .rename_product("alice", &product.id, &format!("Thing {w}.{p}.{r}"))
                        .await
                        .unwrap();
                    took.push(at.elapsed().as_micros() as u64);
                }
            }
            took
        });
    }
    let mut took = Vec::new();
    while let Some(done) = tasks.join_next().await {
        took.extend(done.unwrap());
    }
    let elapsed = started.elapsed();
    let cpu = cpu_time() - cpu;
    sampler.abort();
    took.sort_unstable();
    let at = |q: f64| took[((took.len() - 1) as f64 * q) as usize] as f64 / 1000.0;

    for worker in workers {
        worker.shutdown().await.unwrap();
    }
    cluster.shutdown().await;
    let full = sizes(&path);
    let written = number(&path, "SELECT CAST(COUNT(*) AS REAL) FROM event").await;
    let payload = number(
        &path,
        "SELECT AVG(LENGTH(data) + LENGTH(metadata)) FROM event",
    )
    .await;
    let grown: u64 = full.iter().sum::<u64>() - empty.iter().sum::<u64>();

    println!("store at {}", path.display());
    println!(
        "{written:.0} events by {writers} writers, {subscriptions} subscriptions, in {:.1} s: {:.0} commands/s",
        elapsed.as_secs_f64(),
        took.len() as f64 / elapsed.as_secs_f64(),
    );
    println!(
        "disk: {:.0} bytes per event (data + metadata: {payload:.0}); file {:.1} MiB, write-ahead log {:.1} MiB, commit log {:.1} MiB at the end",
        grown as f64 / written,
        mib(full[0]),
        mib(full[1]),
        mib(full[2]),
    );
    println!(
        "logs at their peak: write-ahead log {:.1} MiB, commit log {:.1} MiB",
        mib(peaks[0].load(Ordering::Relaxed)),
        mib(peaks[1].load(Ordering::Relaxed)),
    );
    println!(
        "a command took: p50 {:.1} ms, p99 {:.1} ms, p99.9 {:.1} ms, max {:.1} ms; {} over 100 ms, {} over 1 s",
        at(0.5),
        at(0.99),
        at(0.999),
        at(1.0),
        took.iter().filter(|t| **t > 100_000).count(),
        took.iter().filter(|t| **t > 1_000_000).count(),
    );
    println!(
        "processor: {:.1} s over the run, {:.2} cores, {:.2} ms per command",
        cpu.as_secs_f64(),
        cpu.as_secs_f64() / elapsed.as_secs_f64(),
        cpu.as_secs_f64() * 1000.0 / took.len() as f64,
    );
    if std::env::var_os("SIZING_DIR").is_none() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
