# Proposals

A proposal is a change to a shop that something prepared and a person has
to read before it applies: a text a model drafted, a price a rule
suggests, a category an agent wants to create, or a change a member wrote
who may not edit the catalog themselves. Nothing a proposal says is
written until a member approves it.

The domain lives in `crates/proposals` (`timada-proposals`); what a kind of
proposal means, and how it is applied, in `dashboard/src/proposals/`.

## What a proposal holds

| Field | What it says |
|---|---|
| kind | What it is about: `product`, `category`, `new_category`, `ads_campaign`, and for the ads issues `ads_bid`, `ads_exclusion`, `ads_negative_term` |
| subject | The product's or the category's id; for a category to create, the category above it (empty for the top level) |
| changes | One to eight fields, each with the value it held when the proposal was prepared (`before`) and the value it would take (`after`) |
| reason | Why: the rule and the numbers it read, or the note of whoever prepared it |
| source | What prepared it (`person`, `writer` for a [text draft](writer.md), a rule's or an agent's name). The waiting list is grouped by it |
| prepared by | The member, or the name the rule or the agent acts under |
| valid until | After it the proposal can be refused, no longer approved. Thirty days by default, ninety at most |

The fields a kind may change:

| Kind | Fields |
|---|---|
| `product` | `name`, `description`, `meta_title`, `meta_description`, `price`, and `price:{variant id}` for one variant |
| `category` | `name`, `description`, `body`, `meta_title`, `meta_description` |
| `new_category` | `name`, `meta_title` |
| `ads_campaign` | the campaign's settings (`name`, `daily_budget`, `bid`, `countries`, …); the subject is the kind of campaign, `shopping` or `brand`. See [the first campaign](google.md#the-first-campaign-691) |
| the other ads kinds | whatever the host names them; nothing applies them yet |

A value is kept as a person reads and types it: a text as it is, a price
as `19.90` in the product's currency. It is normalized the way the command
that writes it would normalize it before it is recorded, so what a
proposal says it would write is what the shop holds once it is applied.

## Its life

```
Prepared ──► Approved ──► Applied
   │             ├──────► FoundOutOfDate
   │             └──────► ApplyFailed
   ├──► Refused
   └──► FoundOutOfDate
```

- **Prepared.** `crate::proposals::prepare` reads what the subject holds
  and records each change against it. A field that would take what it
  holds already is left out; a proposal with nothing left is refused.
- **Approved.** A member says yes, to the values prepared or to the ones
  they typed instead. The event carries the values approved and who
  approved them. A field set back to what the shop holds leaves the
  proposal.
- **Applied.** The saga `timada-dashboard/proposal-changes` sees
  `Approved` on every node of the work group and makes each change through
  the command the product or category page uses, under the name of the
  member who approved. So an approved change meets the checks a typed one
  does, and the product's own history names a person.
- **Out of date.** Before a change is made, each field is read again. One
  that holds neither `before` nor `after` was changed since the proposal
  was prepared: nothing is written over it and the proposal ends. The same
  check runs when the member clicks Approve, so they are told at once.
- **Not applied.** The shop refused the change (a product archived
  meanwhile, a slug taken): the reason is kept on the proposal.
- **Refused.** A member said no, with an optional reason.

A proposal past its date stays in the waiting list, marked *Expired*,
until someone refuses it: it is not dropped silently.

Applying is safe to repeat. A field that already holds the approved value
is passed over, and a category to create goes by an id made of the
proposal's, so two nodes that see the same approval, or one that stopped
half-way, end on the same values.

## Permissions

| Permission | Lets a member |
|---|---|
| `proposals:prepare` | Write a change for someone else to approve ("Suggest a change" on a product or a category), and see what waits |
| `proposals:approve` | Approve, edit or refuse what waits |

Approving also asks for the permission the change itself needs
(`products:manage` for the three catalog kinds, `ads:manage` for a
campaign): a proposal is not a way around it. A member who holds `proposals:approve` alone can read and
refuse. Owners hold everything.

## Pages

- `/t/{slug}/proposals`: what waits, grouped by what prepared it, with the
  values before and after. Tick rows and approve or refuse them together,
  or answer every row a group shows. The *Answered* tab keeps what was
  approved, refused or ended, and by whom.
- `/t/{slug}/proposals/{id}`: one proposal beside what the shop holds
  now. The values can be edited before approving.
- `/t/{slug}/proposals/new?kind=…&subject=…`: a change written by hand.

The lists come from the read model `proposals` (rebuilt with
`timada-dashboard rebuild proposals`); the page of one proposal reads its
stream.

## The notice

When a proposal is prepared by someone who may not approve it, the members
who may are mailed that changes wait, with a link to the list. One mail a
day per shop at most, however many proposals arrive: the notice is a
stream keyed by the UTC day, and the node that claims it first sends, as
for the contact-form notices.

## Preparing from code

A rule, a writer or an agent prepares through the same function the form
uses:

```rust
crate::proposals::prepare(
    &store,
    tenant_id,
    actor,              // who the proposal says prepared it
    Draft {
        kind: KIND_PRODUCT.to_owned(),
        subject_id: product_id,
        values: vec![(FIELD_META_TITLE.to_owned(), title)],
        reason: "Wording for the keyword \"gants moto été\".".to_owned(),
        source: "writer".to_owned(),
        valid_until: None,
    },
    now,
)
```

A new kind needs no new event: add it to `KINDS`, name its fields in
`is_field_of`, and teach `dashboard/src/proposals` to read it
(`subject`), check a value (`normalized`) and write it (`apply`).
