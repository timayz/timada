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
`a8fc4c6`; the writes, the logs and the subscriptions were measured again on
`b03d06d` and on `11a31fd`, which each changed them (200,000 events on the
memory filesystem, 40,000 on the disk). A cluster adds the replicas' journaling and the network to every write;
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
  checkpoint, which the store runs by itself every 256 KiB of it since evento
  `11a31fd` (every 4 MB before, timayz/evento#544): it peaked at 0.7 MiB.
  The write-ahead log (`-wal`) peaked at 1.5 MiB and is emptied by the same
  checkpoint. `timada_store_bytes{file=…}` on `/metrics` gives the three sizes:
  a commit log that keeps growing says the checkpoints fail.
- **Free space:** budget three times the store per node: the store, a
  `snapshot save` of it, and room to grow.
- **A copy while serving** (`snapshot save` under a running server, #593)
  costs what the files take to read: 0.2 s for 360 MiB on the memory
  filesystem, forty copies during a run of 200,000 commands slowing no command
  and failing none. No checkpoint runs while it
  lasts, so the writes made meanwhile stay in the commit log and are folded in
  together when it ends, every write waiting for that: count a second of wait
  per 13 MiB of log, a mebibyte being about a thousand events. A copy that
  lasts a minute on a node writing a hundred events a second leaves 6 MiB.
  On `11a31fd`, forty copies during the same run: the longest took 0.15 s, the
  write-ahead log peaked at 4.9 MiB instead of 1.5, and no command took over
  0.3 s.
  (`SIZING_COPY=<n>` takes n copies during the run and opens each.)

## Writes

| Where the store is | Commands/s | p50 | p99 | Longest |
|---|---|---|---|---|
| Memory filesystem (a sync is free) | 1,113 | 12 ms | 38 ms | 297 ms |
| NVMe, ext4 | 238 | 49 ms | 0.7 s | 2.5 s |

- **The disk's sync time is still the ceiling**, four times higher than it
  was: 36 commands a second on the same disk before evento `b03d06d`, when a
  write synced the commit log about four times, one after the other, and
  concurrent writes did not share the syncs (timayz/evento#530). They now
  share them, so the throughput follows the number of writers. Measure the
  volume's sync time before anything else, and prefer a disk with power-loss
  protection, whose syncs are short.
- **1,000 orders a day is far below it** on average (an order is some tens of
  events), but a burst queues: with 20 writers at once on the disk, one
  command in nine waited over 100 ms and 0.7 % over a second.
- **A checkpoint stops the writes** for a moment, and the reads with them. It
  lasts as long as the commit log it folds in is large, so since `11a31fd` the
  store folds it in every 256 KiB instead of every 4 MB: shorter stops, more
  often, the same time in all. On the memory filesystem the 99th percentile
  went from 361 ms to 38 ms and the longest command from 560 ms to 297 ms;
  0.3 % of the commands took over 100 ms (2 % before) and none over a second.
  The pause does not grow with the store: the same share at 360,000 events as
  at 1.8 million (on `a8fc4c6`).
- **The throughput does not fall as the store grows**: 587 commands/s at
  360,000 events, 579 at 1.8 million (on `a8fc4c6`).

## Subscriptions that follow none of the writes

| Idle subscriptions | Commands/s | Cores busy | Processor per command |
|---|---|---|---|
| 0 | 1,113 | 2.5 | 2.2 ms |
| 47 | 1,047 | 2.6 | 2.5 ms |

A node runs 47 workers (more with `server.also_serves` and
`server.also_serves_no_region`, one more copy of each saga per region served).
A write now wakes only the subscriptions that follow one of its events
(timayz/evento#531, in `b03d06d`); before, every write woke them all, and 47
of them doing nothing cost 37 ms of processor per command and cut the
throughput to a third.

What is left is the look each subscription takes at the store every 250 ms
whether or not anything was written: 188 looks a second for 47 subscriptions.
On `b03d06d` a look that found the store at a checkpoint kept a thread busy
until it ended (timayz/evento#536); since `11a31fd` it sleeps through it, and
so do the pages' reads (`timada_core::db`). The table above is one run each on
`11a31fd`: the 47 cost 0.3 ms of processor per command, a tenth of a core.
The figures below are from `b03d06d`, when they cost over a core.

| 47 subscriptions | Commands/s | Processor per command |
|---|---|---|
| Looking every 250 ms | 651 to 933 (four runs) | 4.6 to 5.0 ms (three of them) |
| Never looking (`SIZING_POLL_MS=3600000`) | 912, 1009 | 3.3 ms |

- **While writes run a look costs about 6 ms of processor**, over one core
  for the 47 of them. With nothing written it costs 0.8 ms, 0.15 core in all,
  the same at 4,000 events as at 200,000 (`SIZING_IDLE_SECS`).
- The throughput and the tail move more from run to run than the processor
  does: 0, 79 and 260 commands of 100,000 took over a second in three runs
  with the looks, 0 and 20 in two runs without.
- On `b03d06d` that was one core per node under load, and again per extra
  region served; on `11a31fd` count a tenth of one.

## Not measured

- The same on three nodes, and under the mix of a real shop (the load test,
  `docs/loadtest.md`, has the shoppers; its figures predate these).
- The storefront connections: the store API tells every connection of every
  write, whatever its shop, before `b03d06d`; a connection that serves one
  shop is now told of that shop's writes alone.
- The sign-in and contact limits ask every other node at each check
  (`rate_limit.rs`): three requests per check on three nodes.
- Restoring a copy taken while a cluster's node served (#597).
