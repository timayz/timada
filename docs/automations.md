# Automations

A shop's team draws *scenarios*: when something happens in the shop (the
**trigger**), do these **steps**. The steps form a tree: a lane under the
trigger, and a condition holding a Yes lane and a No lane of steps of its own.
A condition ends its lane: the run goes down its Yes or its No lane, each to
its own end, and nothing comes after it. A condition put between two steps
takes the steps after it into its Yes lane.

This page covers how a scenario is kept and edited. What runs it comes with
the runs.

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
  `events.lock`, so the catalog grows freely.
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
