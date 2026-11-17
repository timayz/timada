# Sizing a node

What a node's store costs on disk and per write, measured (#477). The numbers
come from `dashboard/tests/sizing.rs`, an ignored test that runs catalog
commands on one node over a store opened as `serve` opens it:

```
cargo test -p timada-dashboard --test sizing -- --ignored --nocapture
SIZING_COMMANDS=1000000 SIZING_DIR=/mnt/data/sizing cargo test ... # on the disk to measure
SIZING_SUBSCRIPTIONS=47 cargo test ...                             # with idle subscriptions
SIZING_NODES=3 cargo test ...                                      # on a cluster of three
```

Each command writes two small events (a product's and its slug's claim), about
60 bytes of data and metadata each. The runs below are from 2026-11-14 on a
12-core machine, Turso 0.8.3, the dev profile (the dependencies optimised),
20 writers, one node without peers. The disk figures are from evento alpha
`a8fc4c6`; the writes, the logs and the subscriptions were measured again on
`b03d06d` and on `11a31fd`, which each changed them (200,000 events on the
memory filesystem, 40,000 on the disk). A cluster adds the replicas'
journaling and the network to every write: these are the floor, and
[Three nodes](#three-nodes) has what the two other nodes add.

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

## Three nodes

The same commands on a cluster of three (#603, `SIZING_NODES=3`): the three
nodes are three processes on the same 12-core machine, the 20 writers all on
node 0, every write journaled by the three. Runs of 2026-11-16 on evento
`11a31fd`, the dev profile, 200,000 events on the memory filesystem and
40,000 on the disk, with the one-node runs of the same day beside them.

| Where the stores are | Nodes | Commands/s | p50 | p99 | Longest |
|---|---|---|---|---|---|
| Memory filesystem | one | 1,133 | 12 ms | 37 ms | 292 ms |
| Memory filesystem | three | 562 to 574 (four runs) | 35 ms | 96 to 118 ms | 256 to 288 ms |
| NVMe, ext4 | one | 291, 300 | 49 ms | 0.16 s | 1.9 s, 2.6 s |
| NVMe, ext4, the three on the one disk | three | 77, 80 | 0.21 s | 1.7 s | 7.3 s, 8.2 s |

| Memory filesystem | Processor per command | Cores busy | Write-ahead log, peak | Commit log, peak |
|---|---|---|---|---|
| One node | 2.2 ms | 2.5 | 5.1 MiB | 0.7 MiB |
| Three: the node that takes the writes | 3.1 ms | 1.7 to 1.8 | 1.4 to 3.6 MiB | 0.5 MiB |
| Three: each of the two others | 1.8 ms | 1.0 | 1.3 to 3.6 MiB | 0.5 to 0.6 MiB |

- **Half the commands a second, three times the wait and three times the
  processor.** With syncs free, a command takes 35 ms instead of 12 and costs
  6.6 ms of processor over the three nodes instead of 2.2: 3.1 ms where it is
  taken, 1.8 ms on each node that only journals and applies it. A node that
  takes no writes itself is busy at 60 % of the one that takes them all.
- **The throughput does not fall as the stores grow**: 574 commands a second
  to 200,000 events, 549 to a million, 537 to two million, the processor per
  command the same throughout.
- **Every node holds the same store and the same logs.** 951 to 975 bytes per
  event on each of the three, as on one node; the two logs peak no higher
  than on one node, on the node written to and on the others alike.
- **On a disk the syncs add up.** The three nodes of these runs share one
  NVMe disk, so every write waits for three journals synced on it: 80
  commands a second against 300 for one node, every command over 100 ms and
  2 % over a second (one node: 7 % and 0.3 %). Three machines with a disk
  each sync side by side, not one behind the other: this row is the worst
  case, not what they will do, and the one to expect lies between the two
  rows. The processor per command is the same as on the memory filesystem
  (3.6 ms and 2.0 ms).
- **About one command in 500,000 took 5 s** with nothing wrong on any node
  (two of 100,000 in one run, four of a million in another, none in five
  other runs; none failed). Five seconds is how long evento waits for the
  answers to a write before it gives up on them; what was waited for is not
  known yet (timayz/evento#549).

### The 47 subscriptions on every node

| Three nodes | Commands/s | The node written to | Each other node |
|---|---|---|---|
| No subscription | 564 | 3.1 ms, 1.7 cores | 1.8 ms, 1.0 core |
| 47 idle subscriptions on each | 526 | 3.4 ms, 1.8 cores | 2.1 to 2.2 ms, 1.1 cores |

Subscriptions that follow none of the writes cost a node the same whether
the writes are its own or arrive from another node: 0.3 to 0.4 ms of
processor per command, a tenth of a core at this rate, on each of the three.
(One node, the same day: 1,133 commands a second without them, 1,114 with,
2.2 and 2.35 ms.)

### A copy while the cluster serves

Forty copies of one node's store (`snapshot save`, `SIZING_COPY=40`, taken on
the node `SIZING_COPY_ON` names) during a run of 100,000 commands, each copy
opened and read afterwards:

| Copies taken on | Commands/s | p99 | The node written to | Each other node | Longest copy |
|---|---|---|---|---|---|
| none | 564 | 113 ms | 3.1 ms | 1.8 ms | |
| node 0, the one written to | 564 | 112 ms | 3.15 ms | 1.8 ms | 0.11 s for 180 MiB |
| node 1 | 562 | 118 ms | 3.1 ms | 1.8 ms | 0.11 s for 180 MiB |

A copy costs the cluster nothing that these runs can show, wherever it is
taken: the same throughput, the same wait, the same processor on the node
copied and on the two others, and logs that peak no higher (the copy holds
the checkpoints of its own node alone, for a tenth of a second here; a copy
that lasts longer leaves that node's commit log to fold in, as above, while
the two others keep checkpointing).

### A new cluster over one node's copy

`snapshot restore --from-one-node` (#597) puts one node's copy under every
node, and they begin a new cluster over it (`SIZING_FOUND=1`): each node reads
every event once before it serves. From the three started at once to the
three taking writes, on the memory filesystem:

| Events in the copy | Store file | The file put back, per node | Until the three take writes |
|---|---|---|---|
| 200,004 | 184 MiB | 0.1 s | 1.8 s |
| 1,000,004 | 908 MiB | 0.6 s | 7.6 s |
| 2,000,004 | 1,814 MiB | 1.0 s | 16.2 s |

- **Count 8 s per million events**, 9 s per GiB of store, in step with the
  size; 20 to 30 million events are three to four minutes. The three nodes
  are ready within a few hundredths of a second of each other, and the first
  write after it takes what any write takes (5 to 8 ms alone).
- On the disk, 40,004 events (37 MiB) took 0.4 s; a store larger than the
  machine's memory will be read at the disk's pace, which these runs do not
  show.

## The limits held across the nodes

The sign-in, code, contact, newsletter and cart limits are held for the whole
cluster: before letting a client through, a node asks every other node what
it let through (`dashboard/src/rate_limit.rs`). `make limits`
(`scripts/limits.sh`) floods the password sign-in of one node, alone and then
as one of three, and prints what each node spent. The figures are from
2026-11-16 (#602): release build, 64 senders, the three nodes and the sender
on the same 12-core machine.

| A sign-in that is | Answers/s | p50 | Processor, the node asked | Each other node |
|---|---|---|---|---|
| refused, on one node alone | 98,000 to 110,000 | 0.5 ms | 0.04 ms | |
| refused on the node's own count, three nodes | 98,600 | 0.6 ms | 0.04 ms | nothing |
| refused on the others' counts, three nodes | 30,200 | 1.9 ms | 0.14 ms | 0.035 ms |
| let through to the password check | 145 to 153 | 0.4 s | 70 ms | under 0.2 core in all |

- **Asking the two others costs 0.1 ms of processor** on the node that asks
  and 0.035 ms on each node asked, and 1.3 ms of wait with 64 at once. A
  refusal that asks is three and a half times dearer than one that does not,
  and 30,000 of them a second on one node keep a core busy on each of the two
  others.
- **A node whose own count is full asks nobody** (since #602; before, every
  refusal asked): a flood on one node costs the others nothing. A client the
  balancer spreads over the nodes is still asked about by the nodes that did
  not let it through to the limit themselves: 40,000 refusals a second over
  the three kept 2.4 cores busy on each of two nodes and 1.5 on the third.
- **The password check is what a sign-in flood costs**, not the limit: 70 ms
  of processor each, and 1.5 GiB of memory for 64 at once. 150 a second fill
  the twelve cores. The limits are what holds it back (30 tries per address
  and 10 per account in 15 minutes, `[auth]`).
- **A node that is gone costs nothing**: it refuses the connection at once
  (69,000 refusals a second over the two left).
- **A node that takes connections and answers nothing** (stopped where it
  stands, or its machine unreachable) is waited for 2 s once, by the checks
  under way when it stops answering. It is then left alone, its passes
  counted for nothing, and asked on the side every 5 s until it answers:
  91,000 refusals a second over the two others with the third stopped, the
  longest 2.2 s. Before #602 every check waited the 2 s for as long as the
  node stayed that way: 32 sign-ins a second for 64 senders, each 2.0 s, and
  at 200 arriving a second the node asked held 900 files open.

## Not measured

- Three nodes on three machines, each with its own disk and a network
  between them: every three-node figure above is from one machine.
- The mix of a real shop on this pin. The load test (`docs/loadtest.md`) has
  the shoppers and prints its own figures; none are kept in a document, and
  the last runs noted (#285 to #289) predate evento `b03d06d` and `11a31fd`.
- The storefront connections: the store API tells every connection of every
  write, whatever its shop, before `b03d06d`; a connection that serves one
  shop is now told of that shop's writes alone.
