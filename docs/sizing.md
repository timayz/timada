# Sizing a node

What a node's store costs on disk and per write, measured (#477). The numbers
come from `dashboard/tests/sizing.rs`, an ignored test that runs catalog
commands on one node over a store opened as `serve` opens it:

```
cargo test -p timada-dashboard --test sizing -- --ignored --nocapture
SIZING_COMMANDS=1000000 SIZING_DIR=/mnt/data/sizing cargo test ... # on the disk to measure
SIZING_SUBSCRIPTIONS=47 cargo test ...                             # with idle subscriptions
```

Each command writes two small events (a product's and its slug's claim), about
60 bytes of data and metadata each. The runs below are from 2026-11-14 on a
12-core machine, evento alpha `a8fc4c6`, Turso 0.8.3, the dev profile (the
dependencies optimised), 20 writers, one node without peers. A cluster adds the
replicas' journaling and the network to every write; these are the floor.

## Disk

| Events | Store file | Bytes per event |
|---|---|---|
| 360,004 | 329 MiB | 962 |
| 1,800,004 | 1,638 MiB | 956 |

- **An event costs its payload plus about 900 bytes** (the row, its indexes and
  the consensus journal's row), and the cost per event does not move with the
  size of the store. Budget 1.3 to 1.5 KB for an event of a few hundred bytes:
  20 to 30 million events are 26 to 45 GB, before the read models, the search
  indexes and the pictures.
- **The file never shrinks.** Nothing vacuums it.
- **The two logs stay small.** The commit log (`-log`) is emptied at each
  checkpoint, which Turso runs by itself every 4 MB of it: it peaked at 4.2 MiB.
  The write-ahead log (`-wal`) peaked at 28 MiB and is emptied by the same
  checkpoint. `timada_store_bytes{file=…}` on `/metrics` gives the three sizes:
  a commit log that keeps growing says the checkpoints fail.
- **Free space:** budget three times the store per node: the store, a
  `snapshot save` of it, and room to grow.

## Writes

| Where the store is | Commands/s | p50 | p99 | Longest |
|---|---|---|---|---|
| Memory filesystem (a sync is free) | 579 | 27 ms | 345 ms | 450 ms |
| NVMe, ext4 | 36 | 483 ms | 2.2 s | 4.6 s |

- **The disk's sync time is the ceiling.** A write syncs the commit log about
  four times, one after the other, and concurrent writes do not share the
  syncs: 7,476 syncs for 1,804 events with one writer, 5,592 with twenty
  (`strace -f -y -e trace=fsync`). At 3.5 ms a sync, that is about 70 events a
  second per node whatever the number of writers (timayz/evento#530). Measure
  the volume's sync time before anything else, and prefer a disk with
  power-loss protection, whose syncs are short.
- **1,000 orders a day is far below it** on average (an order is some tens of
  events), but a burst queues: with 20 writers at once a command waits half a
  second.
- **A checkpoint stops the writes** for a moment. On the memory filesystem
  2 % of the commands took over 100 ms and none over half a second, at 360,000
  events as at 1.8 million: the pause does not grow with the store. On the
  disk, behind the queue of syncs, 4 % took over a second.
- **The throughput does not fall as the store grows**: 587 commands/s at
  360,000 events, 579 at 1.8 million.

## Subscriptions woken by every write

| Idle subscriptions | Commands/s | Cores busy | Processor per command |
|---|---|---|---|
| 0 | 587 | 1.9 | 3.2 ms |
| 47 | 197 | 7.3 of 8 | 37.0 ms |

Every write wakes every subscription of the node for a pass, whether or not
it follows the event. A node runs 47 workers (more with `server.also_serves`
and `server.also_serves_no_region`, one more copy of each saga per region
served): doing nothing, they cost eleven times the processor of the writes
and cut the throughput to a third, about 0.36 ms per empty pass
(timayz/evento#531). On a real disk the syncs hide it at first: at 70 events a
second the passes take about 1.2 cores. Count that per node, and again per
extra region served.

## Not measured

- The same on three nodes, and under the mix of a real shop (the load test,
  `docs/loadtest.md`, has the shoppers; its figures predate these).
- The storefront connections: the store API tells every connection of every
  write, whatever its shop (in timayz/evento#531).
- The sign-in and contact limits ask every other node at each check
  (`rate_limit.rs`): three requests per check on three nodes.
- A backup while serving: `snapshot save` wants the node stopped (#593).
