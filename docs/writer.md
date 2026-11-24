# Text drafts

Two hundred products and twenty categories each need a name, a description
and what search engines show, written for the keyword they are to be found
by. An imported product arrives with its supplier's text, which cannot be
published as it is. The writer drafts those texts with a model; a person
corrects and approves them. Nothing a model wrote is ever published by
itself: a draft is a [proposal](proposals.md).

The domain lives in `crates/writer` (`timada-writer`); the model, the saga
that asks it and the pages, in `dashboard/src/writer/` and
`dashboard/src/app/t/slug/writer/`.

## Setting it up

```toml
[writer]
key = "…"                    # TIMADA__WRITER__KEY in production
# model = "claude-opus-5-5"
# input_usd_per_mtok = 4.0   # what the model's maker charges; set with `model`
# output_usd_per_mtok = 20.0
# daily_usd_per_shop = 5.0
# daily_usd = 50.0
```

Without a key the feature is off: no page offers it and nothing is called.

The model is Anthropic's Claude, behind the `TextWriter` trait
(`dashboard/src/writer/mod.rs`), which is all the rest of the code knows:
another vendor is another implementation of `write(&Ask) -> Answer`. The
one there is (`claude.rs`) makes one `POST /v1/messages` per draft and asks
for the answer as JSON of a fixed shape, so there is nothing to pick apart.
A request the model's safeguards decline is run again by Anthropic on
another of its models where the configured model takes that parameter
(`fallbacks`).

## What a draft is asked from

| Given to the model | Where it comes from |
|---|---|
| The search to write for | Typed with the request; the product's or the category's own name otherwise |
| The language | The shop's main language (`/t/{slug}/languages`); the one the name is written in when the shop never said |
| What the subject says now | Its name, description and search-engine texts: for an imported product, the supplier's wording |
| Facts | A product's brand, its facts table, and the values of its options, as a shopper reads them |
| What a category holds | The categories right below it and a dozen of its products, by name |
| The shop's tone, and what never to claim | The brief on `/t/{slug}/writer`, set by members with `products:manage` |
| The most characters each text may be | The limits the catalog enforces (`core::seo` for the search-engine texts) |

The model is told to use only these, and to write without a fact rather
than supply one.

A product gets a name, a description, a search-engine title and a
search-engine description. A category gets the last three: its name is the
keyword it was made for, and stays the merchant's.

## From request to proposal

```
Requested ──► Started ──► Drafted (the proposal's id, the cost, what to check)
     │            └─────► Failed (why)
     └──────────────────► Failed
```

1. A member with `proposals:prepare` who sees the catalog asks: "Draft the
   texts" on a product's or a category's page, the ticked rows of the
   products list, or a whole category from the writer's page (fifty
   products a click). Each is one `Draft::Requested`; the page answers at
   once.
2. The saga `timada-dashboard/text-drafts` sees the request on every node
   of the work group. The first to write `Started` makes the call; the
   others leave it.
3. Room is held in the day's spending for the most the call may cost. A
   call that would pass the shop's cap or the platform's is not made, and
   the draft fails saying which and when it frees.
4. The model answers. What the call cost is charged in the hold's place.
5. The draft is checked against the shop's data (below) and prepared as a
   proposal, under the name of the member who asked and the source
   `writer`. The draft records the proposal, the cost and what to check.

A draft that fails (no writer, a cap, the model unavailable or declining, a
text the catalog would refuse, a product deleted meanwhile) says why on the
writer's page; the member asks again. A node that stops between `Started`
and the outcome leaves the draft there: it stops counting as on its way
after ten minutes.

## Facts are not invented

The prompt forbids stating a fact the request did not give. That is an
instruction, not a guarantee, so each draft is also checked
(`timada_writer::facts::unfounded`):

- every **figure** in the draft must appear in what the shop holds of the
  subject. `30 cm` in a draft when no `30` is in the name, the description,
  the facts or the options is listed, with its unit;
- every word of a short list of **materials and claims** (leather, cotton,
  steel, waterproof, certified, warranty…, in French and in English) that
  the draft uses and the data does not is listed.

What is listed goes on the draft and into the proposal's reason: "To check,
not found in the shop's data: 30 cm, cuir." The member who approves reads
it before the text.

The check is a net, not a proof. It does not judge whether a fact is true,
and a fact worded outside the list (a plug type, a compatibility) is not
caught. The person approving is the guarantee; the list tells them where to
look first.

## Declared as written by a machine

When a proposal from the writer is applied to a product, the product is
marked as having an AI-written name, description or both
(`declare_ai_writing`), which is what makes the shopping feed send
`structured_title` and `structured_description` as Merchant Center asks.
The marks are on the product's *Shopping feed* tab, where the merchant can
take them off after rewriting a text by hand.

## What it costs

Every call is paid by the platform's account. A day (UTC) has two caps:

| Setting | Caps |
|---|---|
| `writer.daily_usd_per_shop` | What one shop's drafts may cost |
| `writer.daily_usd` | What every shop's drafts may cost together |

A call's cost is reckoned from the tokens the model says it read and wrote,
at the two prices in the configuration: the API reports tokens, not
dollars, so the prices have to be set with the model. A draft comes to a
few cents with the default model.

Room is held before each call at the most it may cost (16,000 tokens each
way, 0.384 USD at the default prices), so a cap is never passed and the
last calls before it are refused while a little of it is left. The day is
one stream in the platform's tenant (`SpendDay`), written against the
version read, so the count holds over a cluster. A call the model declined
is charged what it cost; one that never got an answer is not.

The writer's page shows what the shop spent today. A request re-run on
Anthropic's fallback model is priced as the configured model's, which is
close enough for a cap and not an invoice.
