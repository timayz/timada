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
12-core machine, Turso 0.8.3, the dev profile (the dependencies optimised),
20 writers, one node without peers. The disk figures are from evento alpha
`a8fc4c6`; the writes and the subscriptions were measured again on `b03d06d`,
which changed both (200,000 events on the memory filesystem, 40,000 on the
disk). A cluster adds the replicas' journaling and the network to every write;
these are the floor.

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
| Memory filesystem (a sync is free) | 947 | 13 ms | 361 ms | 560 ms |
| NVMe, ext4 | 159 | 61 ms | 1.1 s | 2.4 s |

- **The disk's sync time is still the ceiling**, four times higher than it
  was: 36 commands a second on the same disk before evento `b03d06d`, when a
  write synced the commit log about four times, one after the other, and
  concurrent writes did not share the syncs (timayz/evento#530). They now
  share them, so the throughput follows the number of writers. Measure the
  volume's sync time before anything else, and prefer a disk with power-loss
  protection, whose syncs are short.
- **1,000 orders a day is far below it** on average (an order is some tens of
  events), but a burst queues: with 20 writers at once on the disk, a third of
  the commands waited over 100 ms and 1 % over a second.
- **A checkpoint stops the writes** for a moment. On the memory filesystem
  2 % of the commands took over 100 ms and none over a second. The pause does
  not grow with the store: the same share at 360,000 events as at 1.8 million
  (on `a8fc4c6`).
- **The throughput does not fall as the store grows**: 587 commands/s at
  360,000 events, 579 at 1.8 million (on `a8fc4c6`).

## Subscriptions that follow none of the writes

| Idle subscriptions | Commands/s | Cores busy | Processor per command |
|---|---|---|---|
| 0 | 947 | 3.2 | 3.4 ms |
| 47 | 651 | 3.2 | 5.0 ms |

A node runs 47 workers (more with `server.also_serves` and
`server.also_serves_no_region`, one more copy of each saga per region served).
A write now wakes only the subscriptions that follow one of its events
(timayz/evento#531, in `b03d06d`); before, every write woke them all, and 47
of them doing nothing cost 37 ms of processor per command and cut the
throughput to a third. What is left costs a third of the throughput and 1.6 ms
of processor per command, and a longer tail: 260 of 100,000 commands took
over a second (none without the subscriptions), the longest 2.7 s. One run
each; the cause of what is left is not looked into.

## Not measured

- The same on three nodes, and under the mix of a real shop (the load test,
  `docs/loadtest.md`, has the shoppers; its figures predate these).
- The storefront connections: the store API tells every connection of every
  write, whatever its shop, before `b03d06d`; a connection that serves one
  shop is now told of that shop's writes alone.
- The sign-in and contact limits ask every other node at each check
  (`rate_limit.rs`): three requests per check on three nodes.
- A backup while serving: `snapshot save` wants the node stopped (#593).
