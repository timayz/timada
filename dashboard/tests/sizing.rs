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
//! events, as most of a node's sagas do for most writes, and look at the store
//! every `SIZING_POLL_MS` (250, evento's own) all the same; `SIZING_IDLE_SECS`
//! (0) then leaves them that long with nothing written, to cost a look alone. `SIZING_DIR` keeps
//! the store where it says instead of a temporary directory, on the disk to
//! measure.
//!
//! `SIZING_NODES=3` runs the same on a cluster of three (#603): the test
//! starts itself twice more, a process per node so each one's processor is
//! its own, and writes through node 0. The two others run the same
//! subscriptions and report what the writes cost them. `SIZING_COPY_ON` (0)
//! names the node that takes the copies of `SIZING_COPY`, and
//! `SIZING_FOUND=1` then puts one node's copy back under the three, as
//! `snapshot restore --from-one-node` does, and times how long they take to
//! begin a new cluster over it.
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
#[path = "common/ports.rs"]
mod ports;
#[path = "../src/store_copy.rs"]
mod store_copy;

use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use evento::metadata::Event;
use evento::subscription::{Context, SubscriptionBuilder};
use timada_catalog::CreateProduct;
use timada_tenancy::aggregator::Created;
use timada_tenancy::{Command, CreateTenant};

use cluster::{Cluster, Executor};
use config::ClusterConfig;

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

/// A node of the run: its store opened as `serve` opens it, alone or as one
/// of the cluster's.
struct Node {
    path: PathBuf,
    store: db::Store,
    cluster: Arc<Cluster>,
    executor: Executor,
    workers: Vec<evento::subscription::Subscription>,
}

impl Node {
    /// Where node `id` of `nodes` keeps its store under `dir`.
    fn path(dir: &Path, nodes: u64, id: u64) -> PathBuf {
        if nodes == 1 {
            dir.join("timada.db")
        } else {
            dir.join(format!("node{id}")).join("timada.db")
        }
    }

    /// Opens the store at `path`: alone, or as node `id` of `peers`.
    async fn open(path: &Path, among: Option<(u64, &[SocketAddr])>) -> Self {
        let store = db::migrate_store(path).await.unwrap();
        let config = among.map(|(id, peers)| ClusterConfig {
            node_id: id,
            peers: peers.iter().copied().map(Into::into).collect(),
            founders: None,
            listen: None,
            web_peers: Vec::new(),
            secret: None,
            // The settings' own defaults.
            absent_after_secs: 24 * 3600,
            write_backlog: 100_000,
        });
        let (cluster, executor, _) =
            cluster::open(config.as_ref(), None, &[], false, path, store.clone())
                .await
                .unwrap();
        Self {
            path: path.to_path_buf(),
            store,
            cluster,
            executor,
            workers: Vec::new(),
        }
    }

    /// Returns once the node takes writes: at once alone, once it heard
    /// enough of the others on a cluster.
    async fn serving(&self) {
        while !self.cluster.health().serving() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Starts `subscriptions` subscriptions that follow none of the run's
    /// writes and look at the store every `poll`.
    async fn subscribe(&mut self, subscriptions: u64, poll: Duration) {
        for n in 0..subscriptions {
            self.workers.push(
                SubscriptionBuilder::<Executor>::new(format!("sizing/{n}"))
                    .handler(on_tenant_created())
                    .poll_interval(poll)
                    .start(&self.executor)
                    .await
                    .unwrap(),
            );
        }
    }

    /// Stops the node and closes its files.
    async fn close(self) {
        for worker in self.workers {
            worker.shutdown().await.unwrap();
        }
        self.cluster.shutdown().await;
    }
}

/// The two logs' sizes, looked at ten times a second: their peaks, the
/// write-ahead log's then the commit log's.
struct Peaks(Arc<[AtomicU64; 2]>, tokio::task::JoinHandle<()>);

impl Peaks {
    fn watch(path: &Path) -> Self {
        let peaks = Arc::new([AtomicU64::new(0), AtomicU64::new(0)]);
        let sampler = tokio::spawn({
            let (path, peaks) = (path.to_path_buf(), peaks.clone());
            async move {
                loop {
                    let [_, wal, log] = sizes(&path);
                    peaks[0].fetch_max(wal, Ordering::Relaxed);
                    peaks[1].fetch_max(log, Ordering::Relaxed);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        });
        Self(peaks, sampler)
    }

    fn reset(&self) {
        for peak in self.0.iter() {
            peak.store(0, Ordering::Relaxed);
        }
    }

    /// The peaks so far, in MiB.
    fn seen(&self) -> [f64; 2] {
        [0, 1].map(|log| mib(self.0[log].load(Ordering::Relaxed)))
    }
}

impl Drop for Peaks {
    fn drop(&mut self) {
        self.1.abort();
    }
}

/// Another node of a run on three: this test started again as a process of
/// its own (`SIZING_NODE`), told what to do on its input and answering on
/// its output. What it says of itself begins with `node `, and is kept to be
/// printed with the rest.
struct Other {
    child: std::process::Child,
    input: Option<std::process::ChildStdin>,
    output: std::io::Lines<BufReader<std::process::ChildStdout>>,
    said: Vec<String>,
}

impl Other {
    /// Starts node `id` of `peers` over its files under `dir`. It says
    /// `ready` once it takes writes.
    fn start(id: u64, peers: &[SocketAddr], dir: &Path) -> Self {
        let peers: Vec<String> = peers.iter().map(SocketAddr::to_string).collect();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["what_a_store_costs", "--exact", "--ignored", "--nocapture"])
            .env("SIZING_NODE", id.to_string())
            .env("SIZING_PEERS", peers.join(","))
            .env("SIZING_DIR", dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        Self {
            input: child.stdin.take(),
            output: BufReader::new(child.stdout.take().unwrap()).lines(),
            child,
            said: Vec::new(),
        }
    }

    fn tell(&mut self, line: &str) {
        writeln!(self.input.as_mut().unwrap(), "{line}").unwrap();
    }

    /// Waits for the node's line that begins with `word`, and returns the
    /// rest of it.
    fn wait_for(&mut self, word: &str) -> String {
        tokio::task::block_in_place(|| {
            loop {
                let line = self
                    .output
                    .next()
                    .unwrap_or_else(|| panic!("a node ended before saying {word:?}"))
                    .unwrap();
                if let Some(rest) = line.strip_prefix(word) {
                    return rest.trim().to_owned();
                }
                if line.starts_with("node ") {
                    self.said.push(line);
                }
            }
        })
    }

    /// Prints what the node said of itself since the last time.
    fn report(&mut self) {
        for line in self.said.drain(..) {
            println!("{line}");
        }
    }

    /// Has the node stop, and prints its last words.
    fn end(mut self) {
        drop(self.input.take());
        tokio::task::block_in_place(|| {
            for line in self.output.by_ref() {
                let line = line.unwrap();
                if line.starts_with("node ") {
                    self.said.push(line);
                }
            }
            assert!(self.child.wait().unwrap().success(), "a node failed");
        });
        self.report();
    }
}

/// What `SIZING_NODE` runs: node `id` of the cluster of `SIZING_PEERS`, with
/// the run's subscriptions, until its input ends. `start` begins the run it
/// measures, `stop <commands> <seconds>` ends it, `copy <file>` takes a copy
/// of its store.
async fn another_node(id: u64) {
    let opening = Instant::now();
    let peers: Vec<SocketAddr> = std::env::var("SIZING_PEERS")
        .unwrap()
        .split(',')
        .map(|peer| peer.parse().unwrap())
        .collect();
    let dir = PathBuf::from(std::env::var("SIZING_DIR").unwrap());
    let mut node = Node::open(&Node::path(&dir, 3, id), Some((id, &peers))).await;
    let opened = opening.elapsed();
    node.serving().await;
    println!(
        "node {id}: opened in {:.2} s, takes writes after {:.2} s",
        opened.as_secs_f64(),
        opening.elapsed().as_secs_f64()
    );
    node.subscribe(
        knob("SIZING_SUBSCRIPTIONS", 0),
        Duration::from_millis(knob("SIZING_POLL_MS", 250)),
    )
    .await;
    println!("ready");

    let peaks = Peaks::watch(&node.path);
    let mut cpu = cpu_time();
    // The input is read on a thread of its own: a read blocks.
    let (told, mut tellings) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            if told.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    while let Some(line) = tellings.recv().await {
        match line.split_once(' ').unwrap_or((&line, "")) {
            ("start", _) => {
                peaks.reset();
                cpu = cpu_time();
            }
            ("stop", run) => {
                let used = (cpu_time() - cpu).as_secs_f64();
                let (commands, secs) = run.split_once(' ').unwrap();
                let (commands, secs): (f64, f64) =
                    (commands.parse().unwrap(), secs.parse().unwrap());
                let [wal, log] = peaks.seen();
                println!(
                    "node {id}: processor {used:.1} s over the run, {:.2} cores, {:.2} ms per command; logs at their peak: write-ahead log {wal:.1} MiB, commit log {log:.1} MiB",
                    used / secs,
                    used * 1000.0 / commands,
                );
                println!("done");
            }
            ("copy", to) => {
                let at = Instant::now();
                store_copy::copy(&node.store, &node.path, Path::new(to))
                    .await
                    .unwrap();
                println!("copied {}", at.elapsed().as_secs_f64());
            }
            _ => panic!("node {id} was told {line:?}"),
        }
    }
    let path = node.path.clone();
    node.close().await;
    let held = number(&path, "SELECT CAST(COUNT(*) AS REAL) FROM event").await;
    println!(
        "node {id}: holds {held:.0} events in {:.1} MiB",
        mib(sizes(&path).iter().sum())
    );
}

/// Three addresses nothing listens on, for the nodes to talk on.
fn free_addrs() -> Vec<SocketAddr> {
    ports::free_addrs(3)
}

/// Takes a copy of the store of node `on` to `to` while it serves, and
/// returns how long that took.
async fn take_copy(node: &Node, others: &Mutex<Vec<Other>>, on: u64, to: &Path) -> Duration {
    if on == 0 {
        let at = Instant::now();
        store_copy::copy(&node.store, &node.path, to).await.unwrap();
        return at.elapsed();
    }
    let other = &mut others.lock().unwrap()[on as usize - 1];
    other.tell(&format!("copy {}", to.display()));
    Duration::from_secs_f64(other.wait_for("copied").parse().unwrap())
}

/// Puts the store kept at `kept` in place of the one at `path`, marked as
/// what a new cluster begins with: what `snapshot restore --from-one-node`
/// does to a node's store.
fn put_back_to_begin_again(path: &Path, kept: &Path) {
    db::remove_store(path).unwrap();
    cluster::mark_for_founding(path).unwrap();
    for (source, target) in db::store_files(kept).iter().zip(db::store_files(path)) {
        if source.is_file() {
            std::fs::copy(source, target).unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "a measurement: run it alone, with --nocapture"]
async fn what_a_store_costs() {
    if let Ok(id) = std::env::var("SIZING_NODE") {
        return another_node(id.parse().expect("SIZING_NODE is a number")).await;
    }
    let commands = knob("SIZING_COMMANDS", 100_000);
    let writers = knob("SIZING_WRITERS", 20);
    let subscriptions = knob("SIZING_SUBSCRIPTIONS", 0);
    let poll = Duration::from_millis(knob("SIZING_POLL_MS", 250));
    let nodes = knob("SIZING_NODES", 1);
    assert!(nodes == 1 || nodes == 3, "SIZING_NODES is 1 or 3");
    let copy_on = knob("SIZING_COPY_ON", 0);
    assert!(copy_on < nodes, "SIZING_COPY_ON names one of the nodes");
    let found = knob("SIZING_FOUND", 0) != 0;
    assert!(!found || nodes == 3, "SIZING_FOUND needs SIZING_NODES=3");
    let dir = std::env::var("SIZING_DIR").map_or_else(
        |_| std::env::temp_dir().join(format!("timada-sizing-{}", ulid::Ulid::generate())),
        PathBuf::from,
    );
    std::fs::create_dir_all(&dir).unwrap();
    let path = Node::path(&dir, nodes, 0);
    assert!(!path.exists(), "{} holds a store already", path.display());

    // The nodes start together: each waits for the others to take writes.
    let peers = (nodes == 3).then(free_addrs);
    let start_others = || -> Vec<Other> {
        (1..nodes)
            .map(|id| Other::start(id, peers.as_deref().unwrap(), &dir))
            .collect()
    };
    let others = start_others();
    let mut node = Node::open(&path, peers.as_deref().map(|peers| (0, peers))).await;
    node.serving().await;
    let others = Arc::new(Mutex::new(others));
    for other in others.lock().unwrap().iter_mut() {
        other.wait_for("ready");
        other.said.clear();
    }
    let tenants = Command(node.executor.clone());
    let create = || {
        tenants.create_tenant(CreateTenant {
            name: "sizing".into(),
            owner_user_id: "alice".into(),
        })
    };
    // The first write after a whole cluster started may miss its quorum.
    let mut tenant = create().await;
    let asked = Instant::now();
    while tenant.is_err() && nodes > 1 && asked.elapsed() < Duration::from_secs(10) {
        tokio::time::sleep(Duration::from_millis(100)).await;
        tenant = create().await;
    }
    let tenant = tenant.unwrap();
    node.subscribe(subscriptions, poll).await;
    let node = Arc::new(node);
    let executor = node.executor.clone();
    let empty = sizes(&path);

    let peaks = Peaks::watch(&path);
    for other in others.lock().unwrap().iter_mut() {
        other.tell("start");
    }

    let products = commands / COMMANDS_PER_PRODUCT / writers;
    let cpu = cpu_time();
    let started = Instant::now();
    // With `SIZING_COPY=<n>`, n copies of the store taken while it is written
    // (`snapshot save` under a running server), at even steps of the run:
    // how long each took, and the commands that went through meanwhile.
    let done = Arc::new(AtomicU64::new(0));
    let copies = knob("SIZING_COPY", 0);
    let copy = (copies != 0).then(|| {
        let (node, others, done, dir) = (node.clone(), others.clone(), done.clone(), dir.clone());
        tokio::spawn(async move {
            let mut taken = Vec::new();
            for n in 1..=copies {
                while done.load(Ordering::Relaxed) < commands * n / (copies + 1) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                let to = dir.join(format!("copy-{n}.db"));
                let before = done.load(Ordering::Relaxed);
                let took = take_copy(&node, &others, copy_on, &to).await;
                let meanwhile = done.load(Ordering::Relaxed) - before;
                taken.push((took, before, meanwhile, to));
            }
            taken
        })
    });
    // The commands the store refused: none, but for what a copy costs.
    let failed = Arc::new(AtomicU64::new(0));
    let mut tasks = tokio::task::JoinSet::new();
    for w in 0..writers {
        let catalog = timada_catalog::Command::new(executor.clone(), &tenant.id);
        let (done, failed) = (done.clone(), failed.clone());
        tasks.spawn(async move {
            // How long each command took, in microseconds.
            let mut took = Vec::new();
            for p in 0..products {
                let at = Instant::now();
                let created = catalog
                    .create_product(
                        "alice",
                        CreateProduct {
                            name: format!("Thing {w}.{p}"),
                            slug: None,
                        },
                    )
                    .await;
                let product = match created {
                    Ok(product) => product,
                    Err(err) => {
                        eprintln!(
                            "{:?}: a command failed after {:?}: {err}",
                            started.elapsed(),
                            at.elapsed()
                        );
                        failed.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                };
                took.push(at.elapsed().as_micros() as u64);
                done.fetch_add(1, Ordering::Relaxed);
                for r in 1..COMMANDS_PER_PRODUCT {
                    let at = Instant::now();
                    if let Err(err) = catalog
                        .rename_product("alice", &product.id, &format!("Thing {w}.{p}.{r}"))
                        .await
                    {
                        eprintln!(
                            "{:?}: a command failed after {:?}: {err}",
                            started.elapsed(),
                            at.elapsed()
                        );
                        failed.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    took.push(at.elapsed().as_micros() as u64);
                    done.fetch_add(1, Ordering::Relaxed);
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
    for other in others.lock().unwrap().iter_mut() {
        other.tell(&format!("stop {} {}", took.len(), elapsed.as_secs_f64()));
        other.wait_for("done");
    }
    let copy = match copy {
        Some(copy) => Some(copy.await.unwrap()),
        None => None,
    };
    let peaks = peaks.seen();
    // And one with nothing written.
    let idle_copy = if copies != 0 {
        let to = dir.join("idle-copy.db");
        let took = take_copy(&node, &others, copy_on, &to).await;
        Some((took, sizes(&to).iter().sum::<u64>()))
    } else {
        None
    };
    took.sort_unstable();
    let at = |q: f64| took[((took.len() - 1) as f64 * q) as usize] as f64 / 1000.0;

    // What the subscriptions' looks cost with nothing written.
    let idle = Duration::from_secs(knob("SIZING_IDLE_SECS", 0));
    if !idle.is_zero() {
        let before = cpu_time();
        tokio::time::sleep(idle).await;
        let used = (cpu_time() - before).as_secs_f64();
        let looks = subscriptions as f64 * idle.as_secs_f64() / poll.as_secs_f64();
        println!(
            "idle for {idle:?}: {used:.2} s of processor, {:.2} ms per look",
            used * 1000.0 / looks.max(1.0),
        );
    }

    // The copy a new cluster begins with, taken while the three still serve.
    let kept = dir.join("kept.db");
    if found {
        take_copy(&node, &others, copy_on, &kept).await;
    }
    let Ok(node) = Arc::try_unwrap(node) else {
        panic!("the node is still held");
    };
    node.close().await;
    let others = std::mem::take(&mut *others.lock().unwrap());
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
        "{written:.0} events by {writers} writers on {}, {subscriptions} subscriptions (a look every {poll:?}), in {:.1} s: {:.0} commands/s",
        if nodes == 1 {
            "one node"
        } else {
            "node 0 of three"
        },
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
        peaks[0], peaks[1],
    );
    println!(
        "a command took: p50 {:.1} ms, p99 {:.1} ms, p99.9 {:.1} ms, max {:.1} ms; {} over 100 ms, {} over 1 s, {} failed",
        at(0.5),
        at(0.99),
        at(0.999),
        at(1.0),
        took.iter().filter(|t| **t > 100_000).count(),
        took.iter().filter(|t| **t > 1_000_000).count(),
        failed.load(Ordering::Relaxed),
    );
    println!(
        "processor: {:.1} s over the run, {:.2} cores, {:.2} ms per command",
        cpu.as_secs_f64(),
        cpu.as_secs_f64() / elapsed.as_secs_f64(),
        cpu.as_secs_f64() * 1000.0 / took.len() as f64,
    );
    // What the two others said of the run, and what they hold now.
    for other in others {
        other.end();
    }
    for (took, before, meanwhile, to) in copy.unwrap_or_default() {
        let size: u64 = sizes(&to).iter().sum();
        // The copy opens as a store, and holds what was written before it:
        // two events a command.
        let held = number(&to, "SELECT CAST(COUNT(*) AS REAL) FROM event").await;
        // Another node may not hold yet what node 0 was just told is written.
        assert!(
            copy_on != 0 || held >= (before * 2) as f64,
            "{} holds {held} events, {before} commands were in before it",
            to.display()
        );
        println!(
            "a copy of node {copy_on} while written, {before} commands in: {:.2} s for {:.1} MiB ({:.0} MiB/s), {:.0} commands/s meanwhile; it holds {held:.0} events",
            took.as_secs_f64(),
            mib(size),
            mib(size) / took.as_secs_f64(),
            meanwhile as f64 / took.as_secs_f64(),
        );
    }
    if let Some((took, size)) = idle_copy {
        println!(
            "a copy with nothing written: {:.2} s for {:.1} MiB, {:.0} MiB/s",
            took.as_secs_f64(),
            mib(size),
            mib(size) / took.as_secs_f64(),
        );
    }
    if found {
        // Every node stopped, the one copy goes under the three, and they
        // start together.
        let kept_events = number(&kept, "SELECT CAST(COUNT(*) AS REAL) FROM event").await;
        let putting = Instant::now();
        for id in 0..nodes {
            put_back_to_begin_again(&Node::path(&dir, nodes, id), &kept);
        }
        println!(
            "a new cluster over the copy of node {copy_on}, {kept_events:.0} events in {:.1} MiB, put back in {:.2} s:",
            mib(sizes(&kept).iter().sum()),
            putting.elapsed().as_secs_f64(),
        );
        let opening = Instant::now();
        let mut others = start_others();
        let node = Node::open(&path, peers.as_deref().map(|peers| (0, peers))).await;
        let opened = opening.elapsed();
        node.serving().await;
        println!(
            "node 0: opened in {:.2} s, takes writes after {:.2} s",
            opened.as_secs_f64(),
            opening.elapsed().as_secs_f64()
        );
        for other in &mut others {
            other.wait_for("ready");
            other.report();
        }
        let at = Instant::now();
        let first = timada_catalog::Command::new(node.executor.clone(), &tenant.id)
            .create_product(
                "alice",
                CreateProduct {
                    name: "After the new beginning".into(),
                    slug: None,
                },
            )
            .await;
        println!(
            "the first write after it took {:.1} ms{}",
            at.elapsed().as_secs_f64() * 1000.0,
            first
                .err()
                .map_or(String::new(), |err| format!(" and failed: {err}")),
        );
        node.close().await;
        for other in others {
            other.end();
        }
    }
    if std::env::var_os("SIZING_DIR").is_none() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
