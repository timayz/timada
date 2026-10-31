# Automations

A shop's team draws *scenarios*: when something happens in the shop (the
**trigger**), do these **steps**. The steps form a tree: a lane under the
trigger, and a condition holding a Yes lane and a No lane of steps of its own.
A condition ends its lane: the run goes down its Yes or its No lane, each to
its own end, and nothing comes after it. A condition put between two steps
takes the steps after it into its Yes lane.

This page covers how a scenario is kept and edited, then how a published one
runs.

## The definition

A `Scenario` stream (`timada-automations/Scenario`, a ULID per scenario) holds
the scenario's name and its `Definition`, kept whole as JSON in the events:

```json
{
  "schema": 1,
  "trigger": { "kind": "order.placed" },
  "steps": [
    {
      "id": "s1",
      "kind": "condition",
      "settings": { "match": "all", "rules.0.field": "order.total", "rules.0.op": "ge", "rules.0.value": "50" },
      "branches": [
        [{ "id": "s2", "kind": "notify", "settings": { "subject": "A big order" } }],
        []
      ]
    }
  ],
  "next_id": 2
}
```

- **Kinds are strings.** The catalog (`crates/automations/src/catalog.rs`)
  names the triggers and the steps this build knows, what each trigger is about
  (its *subject*: an order, a customer, a subscriber, a review, a message) and
  what a condition may look at for each subject. No enum is frozen in
  `events.lock`, so the catalog grows freely. Today's triggers: `order.placed`
  (paid: an order exists only once paid), `order.fulfilled`, `order.cancelled`,
  `order.refunded` (once per refund), `shopper.registered`,
  `subscriber.confirmed`, `review.submitted`, `review.published`,
  `message.received` and `checkout.abandoned` (a shopper asked to pay and no
  order followed within `[automations]`'s delay). Today's steps: `wait`,
  `condition` (two lanes), `notify` (a note to the shop's members), `email`
  (a message the merchant wrote, to the customer the run is about), `discount`
  (a single-use code a later mail may quote) and `webhook` (what started the
  run, posted signed to an address of the merchant's).
- **Settings are kept as typed.** A step's settings are strings by key, read
  into typed settings by the catalog's `parse_*` functions. A draft never loses
  what was written, even when it does not read yet; the editor shows it next to
  what is wrong.
- **Unknown kinds survive.** A node meeting a kind it does not know (written by
  a newer node) keeps the block and its settings as they are, shows it as an
  unknown block, and refuses to publish the scenario until it is removed. A
  definition of a later `schema` is shown, never edited.
- **Ids are never reused.** Steps are `s1`, `s2`, …; `next_id` remembers the
  last one handed out, so an id removed is never handed out again.
- **Limits:** 50 steps, conditions nested 4 deep, 10 rules per condition, 256
  KiB of JSON, waits of at most 90 days, names of 80 characters.

`Definition::check` lists what keeps a definition from being published (no
trigger, no step, a setting missing or wrong, a condition looking at what the
trigger's subject does not have). A draft may have such problems; only the
hard ones (beyond the limits, a damaged block, a newer format) keep it from
being saved.

## The stream

| Event | What it records |
|---|---|
| `Created { name }` | the scenario, with nothing in it |
| `Renamed { name }` | |
| `Saved { definition }` | the draft, whole |
| `Published { version, definition }` | the draft frozen as version 1, 2, …; it runs |
| `Paused`, `Resumed` | held, then running again |
| `Archived` | deleted from the merchant's view; it cannot be edited again |

Every event carries who did it. The tenant's store is readable by every
storefront key of the tenant (`read_only_store.rs`), so no automation event holds
personal data or a secret.

## The editor

`/t/{slug}/automations/{id}` (`automations:manage`; `automations:read` sees the
flow read-only). The page keeps the **working copy**, not the store:

- One form holds the definition as JSON in a hidden field, with `base` (the
  scenario's version the copy was made from), `selected` (the block whose
  settings are open, or `trigger`) and `moving` (a block picked up to move
  without dragging).
- Every action is a submit button named `op`: `add:<slot>:<kind>`,
  `move:<step>:<slot>`, `pick:<step>` or `place:<kind>` then `drop:<slot>` or
  `cancel`, `up:<step>`, `down:<step>`, `remove:<step>`, `select:<id>`,
  `trigger:<kind>`, `rule-add`, `rule-remove:<i>`, `apply`, `save`. A slot is
  `root.<i>` or `<step>.<branch>.<i>`: the place before the `i`-th step of a
  lane.
- The open block's settings come as `settings.<key>` fields with the block's id
  in `settings_for`, and are applied before every action, so nothing typed is
  lost to a drag.
- The page posts back to itself (`#[page(POST)]`) and comes back with the
  change made. Nothing is written until **Save draft**, which writes `Saved`
  (and `Renamed`) with `original_version(base)`: a copy someone else saved over
  since is refused (`scenario_changed`), and the page keeps the copy on screen.

Nothing in it needs a drag (WCAG 2.2 SC 2.5.7): each slot has a "+" menu of the
blocks (the popover menu, no script needed), each block has Move (the slots then
offer "Move here"), Move up, Move down and Remove, and a palette block pressed
without dragging is picked up the same way.

`editor.js` (next to the page, loaded with `asset!`) adds, on top:

- **Dragging**, with pointer events (mouse, touch, pen): a palette block dropped
  on a slot presses `add:`, a block's handle dropped on a slot presses `move:`.
  Escape cancels.
- **Sending without leaving the page:** the form is posted with `fetch`
  (URL-encoded, as a plain form posts it) and the editor the server sends back
  replaces the old one; a save's redirect is followed.
- **Unsaved work:** a warning before leaving while the copy holds unsaved
  changes, and a copy in the browser's session storage, offered back on the
  next visit.

The script only sets data attributes; the classes they switch on are written in
the Rust markup, which is what the stylesheet is built from.

## Runs

Publishing freezes the draft as the next version (`Published`); Pause and Resume
hold a published scenario and let it go on; Delete archives it. Each event that
is a trigger then starts a **run** of every published scenario of the shop
listening to it: a `Run` stream (`timada-automations/Run`) whose id is a hash of
the scenario and the event, written at version 0, so an event starts a scenario
once however many nodes see it. A run keeps ids only (the scenario, its version,
the trigger, the event, the subject); what a step needs is read from the subject
when the run gets to it.

| Event | What it records |
|---|---|
| `Started { scenario_id, scenario_version, trigger, trigger_event_id, subject_id }` | the run, pinned to the version it started on |
| `Waiting { step, until }` | a wait began |
| `Branched { step, branch }` | a condition chose its lane (0 is Yes) |
| `StepClaimed { step, attempt }` | a step that acts was taken, before it acts |
| `StepSucceeded { step, output }`, `StepSkipped { step, reason }` | it acted, or had nothing to do |
| `StepFailed { step, attempt, reason, retry_at }` | it did not act; without `retry_at` the run ends there |
| `Finished`, `Cancelled { reason }` | the end, or a stop (a member, or the scenario deleted) |

**What starts runs.** One subscription on every node
(`timada-dashboard/automation-triggers`, `automations/triggers.rs`) follows the
trigger events: an order placed, shipped, cancelled or refunded, a customer
account made, a newsletter subscription confirmed, a review submitted or
published, a contact message received. The same subscription keeps
`automations_listeners` from the scenarios' `Published`, `Paused`, `Resumed`
and `Archived`, so the order of the store decides: an event after a publication
starts a run of that version, one before it never does. An event older than an
hour is history being replayed and starts nothing.

**What moves them.** Nothing in the event store fires later, so every node looks
for the runs due on a timer (`[automations] check_secs`, at most
`runs_per_check` runs a look, from `automations_runs`). The runner
(`automations/runner.rs`) loads a run and its version's definition, and a pure
walk (`interpreter::next`) says what comes next: a wait to begin, a condition to
check (on the subject's facts read now, so a condition after a wait sees the
order as it is then), a step to act, or the end. Every move is written against
the run's version: a node that loses the race writes nothing.

- **A step that acts** is claimed first (`StepClaimed`); only the node whose
  claim is written acts. A notice is not sent twice: a claim whose outcome never
  comes (the node stopped) fails the step once its ten-minute lease is over.
- **A paused scenario** holds its runs where they are; resumed, they go on. A
  deleted one cancels them.
- **A skipped step** (nobody to tell) lets the run go on; a failed one ends it.
- **Ended runs** leave `automations_runs` after 90 days; their streams stay.

**Notify the team** mails, on the platform's mail server, the members allowed to
see the run's subject (`orders:read` for an order, `customers:read` for a
customer, `newsletter:read`, `reviews:read`, `support:read`): the merchant's
subject and note, and a link to the subject in the dashboard. Nothing a shopper
wrote travels.

**No loops.** No step makes an event that is a trigger, so a scenario cannot
start itself. A step added later that does (cancelling an order, say) must not
be offered on the trigger it would fire.

The runs of a scenario are listed under `/t/{slug}/automations/{id}/runs`, each
with its steps; members with `automations:manage` cancel one under way.

## Mails to customers

**E-mail the customer** mails whoever the run is about: the buyer of an order
(of the order a review is about, for a review trigger), the customer who made an
account, the subscriber, the visitor who wrote. The merchant writes the subject
and the HTML (and, if they like, the plain text) as templates over the shop's
variables, plus the order's for a scenario about orders or reviews
(`ShopMailKind::AutomationOrder`, else `AutomationCustomer`); each mail must link
to `{{ unsubscribe_url }}`. The editor checks and previews it as it is written.

These mails are marketing, so:

- **They go through the mail server the shop picked for marketing only**, never
  the platform's. Publishing a scenario that mails customers needs
  `newsletter:manage`, a server picked for marketing on `/t/{slug}/mail`, its
  storefront's address (the link points there) and the operator's secret
  (`[secrets] key`, which signs the links). A run that reaches a mail while
  the shop lacks one of them waits for it, up to three days, then fails saying
  which.
- **Anyone can stop them.** The link is `{storefront}/emails/opt-out/{token}`,
  the token being `{opt_out_id}.{tag}`: the id is a hash of the address
  (`timada-automations/OptOut`, which keeps no address), the tag an HMAC of the
  tenant and the id under a key every node derives from `[secrets] key`. Nothing
  is stored to check it, it never expires, and it is worthless for another shop.
  The storefront hands it back to `POST /store/automations/opt-out`; a mail
  client's one-click unsubscribe (RFC 8058) does the same. A member stops (or
  restores) an address on the Automations page, at a customer's request.
- **Who is skipped:** an address that stopped them, one whose account was
  erased, one that left (or was erased from) the newsletter, one whose sealed
  address is gone, and one this scenario already mailed today
  (`timada-automations/MailClaim`, one claim per scenario, address and day): a
  contact form or a checkout, whose addresses nobody checked, is no relay. A
  skipped mail lets the run go on, and the run's log says why.
- Claimed before it goes out, a mail is never sent twice, as for the notices.

## Discount codes

**Create a discount code** makes a single-use code (`max_uses` 1, for anyone)
that ends after the step's number of days: a percent or an amount in a currency,
starting with the step's prefix (`THANKS` gives `THANKS-7KQ2M9PX`). The discount's
id is a hash of the run and the step, and `timada_discounts`'
`create_discount_with_id` returns the discount already made under it, so a retry
or another node makes the same code, never a second one. Its output (the code,
what it takes off, its last day) is what a later **E-mail the customer** quotes:
the default automated mails print it inside `{% if discount %} … {% endif %}`
(`{{ discount.code }}`, `{{ discount.value }}`, `{{ discount.ends }}`).

Unlike a mail, making a code is safe to ask for again: when it fails for a
reason of the store's, the step is tried again after 1 minute, 5 minutes, 30
minutes and 2 hours (`StepFailed` with `retry_at`) before the run fails. A
scenario that makes codes is published by members holding `discounts:manage`.

## Webhooks

**Call a webhook** posts what started the run to an address of the merchant's.
A hook's address is often its only key, and the definition replicates to the
shop's storefronts, so the address is **sealed** in the definition
(`url_sealed`, under a key derived from `[secrets] key`, bound to the tenant,
the scenario and the step); only its host is kept in clear, for the card. The
editor never shows it again once saved.

The call is a JSON `POST` holding ids and no personal data (the run and step,
the trigger, the shop, the scenario and its version, the subject's kind and id):
the merchant's app reads the rest from the store with its key. It carries
`Timada-Signature: t={unix},v1={hex}`, the HMAC-SHA256 of `"{t}.{body}"` under a
secret of the step's own (`whsec_…`, derived from `[secrets] key`, shown in the
step's settings, stored nowhere), and `Idempotency-Key: {run}:{step}`, the same
on every try. Calls take at most `[automations] webhook_timeout_secs` and follow
no redirect; a host that is not public is refused (unless
`mail.allow_private_hosts`, for development), both before the call and at the
address the call dials, so a name that changes its answer in between gets nowhere.
A 2xx is done; no answer, 408, 429
or a 5xx is tried again (as for the codes); another answer fails the step.
Publishing a scenario that calls webhooks needs `api-keys:manage`: like a key, it
hands the shop's events to another system.

## Abandoned checkouts

**When a checkout is left unpaid** starts when a shopper gave their address to
pay for a cart (`Cart::CheckoutRequested`) and no order followed within
`[automations] abandoned_after_secs` (an hour unless set otherwise). Nothing in
the store says "nothing happened", so:

1. The subscription that starts runs also follows the carts: each request is
   recorded in `automations_checkouts` (the latest one per cart), and a cart that
   becomes an order (`Cart::CheckedOut`) is marked ordered there.
2. Every look of the runner finds the requests older than the delay that no order
   followed, in shops with a published scenario on this trigger, and checks each
   against the cart itself (ordered since, or asked again: not left). It writes a
   `timada-automations/AbandonedCheckout` stream (`Detected`, id hashed from the
   cart and the request, at version 0), so a checkout is found left once whichever
   node finds it. Requests older than three days past the delay start nothing.
3. `Detected` is the trigger event: it starts the runs like the others, about the
   cart.

The run's subject is the cart: a condition sees its total (what the shopper was
about to pay), its items and whether it was **ordered since**, read when the run
reaches it, so "Wait 23 hours, then if not ordered since, e-mail a reminder" is a
reminder a day later. A run whose cart becomes an order is cancelled before its
next step. **E-mail the customer** goes to the address of the cart's latest
request, as a mail with nothing of an order (`AutomationCustomer`); linking back
to the cart itself is not offered yet.
