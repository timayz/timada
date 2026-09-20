# Evolving what timada persists

Timada stores events with [evento], encoded with **bitcode**. Bitcode is
positional and packed: there are no field names on disk, no tolerance for an
extra or missing field, and even an enum's discriminant is packed to the number
of variants it had when it was written. Three things are therefore permanent
once a database has seen them:

| Permanent | Where it comes from | Example |
|---|---|---|
| the aggregate name | `#[evento::aggregate(name = "timada-order/Order")]` | renaming the crate or the enum must not change it — that is why it is pinned |
| the event name | the variant's identifier | `OrderPlaced` |
| the layout of the event and of every type nested in it | field order and types | `OrderPlaced { cart_id: String, … }`, `Money { minor: i64, currency: String }` |

`events.lock` writes all of it down, and a test refuses any change to a line
that is already there. The rest of this page is what to do instead.

## The lock

```text
cargo test -p timada-event-lock            # what CI runs
cargo run -p timada-event-lock -- update   # after adding events, types or views
```

`events.lock` has three kinds of lines:

- `event <aggregate>::<Variant> { … }` — **frozen**. Never edited, renamed or removed.
- `type <crate>::<Name> …` — an `Encode` type reachable from an event. **Frozen**
  too, enums included: appending a variant changes how the old ones decode.
- `view <crate>::<Name> rev=<n> { … }` — a snapshotted projection, together with
  the `Encode` types only it uses. It **may change**, provided its
  `.revision(n)` grows.

`update` only ever appends: it refuses to record a change to a frozen line. In
a pull request, a new event is a new line in `events.lock`; an edited line is
a mistake the test already failed on.

`--allow-changes` overrides the refusal. It exists for one case: a shape that
**no deployed database has ever stored** (an event added and reworked inside
the same unreleased branch). Renaming a field is the other harmless edit —
names are not on disk — but the lock cannot tell a rename from a swap of two
fields of the same type, so it asks for the override too; check that order and
types are untouched before using it.

## Changing an event

You cannot. Pick one of these instead.

### 1. A companion event — the event needs *more*

When a fact gains information that not every occurrence has, record the extra
as its own event, committed in the same batch. This is how orders got their
discount and their number without touching `OrderPlaced`:

```rust
let mut write = evento::append(&id);
write.event(&OrderPlaced { /* unchanged */ });
if let Some(order_number) = cmd.order_number {
    write.event(&OrderNumberAssigned { order_number });   // new line in events.lock
}
write.commit(executor).await?;
```

Old streams simply do not have the companion; projections treat its absence as
"none". A consumer of `OrderPlaced` that needs the extra loads the view that
folds both (`OrderDetailsView`) instead of reading the payload alone.

### 2. A new variant — the fact itself has another form

When something can now happen in a way the old event cannot express, add a
variant and handle both. `OrderSettled` sits next to `OrderPaid` because a
zero-total order is paid *without* a payment id; every consumer of `OrderPaid`
handles `OrderSettled` too.

If the new form replaces the old one outright, name it `<Event>V2`, stop
writing the old one, and keep reading it:

```rust
Projection::new::<Payment>()
    .handler(on_payment_refunded())      // still in old streams
    .handler(on_payment_refunded_v2())   // what is written from now on
```

Never reuse a name for a different shape.

### 3. Nothing at all — it was not persisted

Write-side states are `#[evento::snapshot(none)]`: rebuilt from events on
every load, never stored. They change freely — `PaymentState` gained
`refund_reasons` and `QuestionState` gained `customer_id` this way. Reach for
this first: most "I need one more field" is about the state, not the event.

SQL read models are not in the lock either; they evolve with migrations.

### Every new event has strict consumers

Read-model subscriptions and write-side projections are `.strict()`: an event
without a handler or a `.skip::<E>()` fails them. When you add a variant, grep
for the aggregate's other events and add the new one everywhere they appear.
Subscriptions that fail retry forever, so a test that hangs after adding an
event usually means a consumer was missed.

## Changing a value type

A type nested in an event (`Money`, `Address`, `OrderLine`, `PaymentMode`, …)
is as frozen as the event. To evolve one, add a new type, use it in a **new**
event variant, and leave the old type in place for the old events. Adding a
variant to a frozen enum is *not* safe, even at the end.

A type used only by views is not frozen; it is part of those views' shapes.

## Changing a snapshotted view

Views declared with `#[evento::projection(bitcode::Encode, bitcode::Decode)]`
are snapshotted through the executor. Change their shape — a field, or a
variant of an enum only they use — and bump the projection's revision in the
same commit, so snapshots taken with the old shape are dropped rather than
mis-decoded:

```rust
Projection::new::<Shipment>()
    .handler(on_shipment_cancelled())
    .revision(1)      // was absent (0) before `cancelled_reason` was added
    .strict()
```

Then run `update`. The lock fails a changed view whose revision did not move,
and a revision that went backwards. Keep one snapshotted view per file: that is
how the tool knows which `.revision(..)` belongs to which view.

**And one snapshotted view per aggregate.** evento stores a snapshot under
`(aggregate type, revision, id)` — the view is not part of the key. Two
snapshotted views of the same aggregate at the same revision overwrite each
other, and the next load of either fails to decode the other's bytes
(`invalid packing`); inside a subscription that is a handler retried for ever.
A second view of an aggregate is declared without a snapshot:

```rust
#[evento::projection(id = customer_id)]
#[evento::snapshot(none)]
pub struct CompanyIdentityView { /* … */ }
```

(`Customer` had two for a few hours; the address book's revision was bumped
afterwards so that whatever was stored under its key is ignored.)

## Retiring an event

An event that is no longer written still has to be read, for as long as a
database holds one. Keep the variant, keep the handlers, and say so in its doc
comment. Actually deleting it means rewriting the streams that contain it (a
copy-and-replace migration) in every deployed database — not something a
framework can do on its hosts' behalf. The lock treats a removed event as an
error for that reason.

## What is missing: upcasting, in evento

With the rules above, a projection that must understand `FooV1` and `FooV2`
carries two handlers forever. Proper upcasting — translating the old shape into
the new one *before* handlers see it, so V1 handlers can be deleted — cannot be
built in timada: handlers are dispatched by evento on the stored event name.
It would be an evento feature, roughly:

```rust
Projection::new::<Payment>()
    .upcast::<PaymentRefunded, PaymentRefundedV2>(|old| PaymentRefundedV2 {
        amount: old.amount,
        reason: old.reason,
        reference: None,
    })
    .handler(on_payment_refunded_v2())   // the only handler left
```

and the same on `SubscriptionBuilder`, with the upcaster registered once per
aggregate rather than per projection (for instance on the
`#[evento::aggregate]` enum: `#[evento(upcast_to = PaymentRefundedV2)]` on the
old variant, plus a `From` impl). Timada has no `V2` event yet, so nothing is
blocked on it; the day one appears is the day to build this.

[evento]: https://github.com/timayz/evento

## What is missing: a snapshot key per view, in evento

The rule "one snapshotted view per aggregate" above is a workaround. In
`evento-core`'s `projection.rs`, `get_snapshot` and `take_snapshot` key the
stored bytes by `(aggregate_type, revision, id)`. Nothing in that key says
*which* projection the bytes belong to, so two projections of one aggregate
share a slot. The fix belongs in evento: make the projection's identity part
of the key — for instance the type name the `#[evento::projection]` macro
already knows —

```rust
// today
executor.get_snapshot(aggregate_type, revision, id)
// proposed
executor.get_snapshot(aggregate_type, projection_name, revision, id)
```

with the existing rows read under an empty `projection_name` once (or simply
dropped: a snapshot is a cache). Until then a collision is silent at compile
time and only shows when both views are loaded for the same id with a snapshot
taken in between — a test that hangs, not one that fails. A cheaper guard in
the meantime would be for evento to store the projection's name *inside* the
snapshot row and treat a mismatch as "no snapshot" instead of decoding it.
