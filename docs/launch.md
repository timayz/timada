# Launch: the path, the target and the checkpoints

A new shop loses money for weeks by design. Without a target fixed
beforehand and dates to read the numbers at, "is this working" is answered
by mood, and a niche that should be dropped is kept.

`/t/{slug}/checkpoints` (Sales, for `orders:read`) holds both. The domain
is `crates/launch` (`timada-launch`): one stream a shop, in the shop's own
tenant. What reads the numbers and mails the owners is
`dashboard/src/checkpoints.rs`.

## The launch path (#696)

Between "a shop with a name" and "a shop that is advertised" there are some
twenty things to get right, in an order, spread over a dozen pages.
`/t/{slug}/launch` (Sales, for `orders:read`) lists them, each done, to do
or not checked, with a link to where it is put right. The code is
`dashboard/src/launch_path.rs`.

A step reads what timada already holds; nothing is fetched from outside to
draw the page. Where timada cannot know, the step takes a member's word
(`tenant:manage`), kept with who gave it (`Launch::StepConfirmed`, taken
back with `StepUnconfirmed`). A word does not make up for what timada can
tell is missing: with no page published, the legal pages cannot be said to
hold.

| Part | Step | How it is known | Before ads |
|---|---|---|---|
| Niche | A niche study says go | the study linked to the shop (#682) and its verdict | |
| Categories | The shop has its categories | a category shown to shoppers | |
| | Every category at the bottom of the tree holds products | each holds 10 or more (a setting) | |
| | No two categories hold the same products | the categories' product sets | |
| Catalog | The catalog is as large as planned | published products against 200 (a setting) | |
| | No product has the address of a category | product slugs against category slugs | yes |
| | Every product leaves something, and its break-even is known | the margin of every thing on sale, from its cost | yes |
| | No product is left with its supplier's text | a member's word | |
| Shop | The company's details are filled | the shop's company has its legal identity | yes |
| | The legal pages are written for this company | a page is published, and a member's word | yes |
| | The storefront asks before it measures | a member's word | yes |
| | Shoppers have a way to reach the shop that answers | a member's word | |
| | Delivery and returns are stated | a shipping method and the return terms | yes |
| Found | The sitemap and the robots file answer | the shop's address is set, and a member's word | |
| | Search Console is connected and has the sitemap | the site is chosen, and a member's word | |
| Measured | The Google tag is set and counts the sales | its Google Ads id and purchase label | yes |
| | A real test order went through, its purchase counted | a member's word | yes |
| Speed | The home, a category and a product pass Core Web Vitals on a phone | a member's word, with a link to PageSpeed Insights | |
| Feed | Merchant Center accepts every product | the last read of Merchant Center | yes |
| Ads | The first campaign is prepared | a proposal waits, or the campaign exists | |
| | The first campaign runs | its live date | |

The steps marked "before ads" must hold before the first campaign is
prepared: the campaigns page lists the ones that do not among what the
campaign waits for, and its form does not show until they hold.

Delivery and returns are "the same as in the structured data" by
construction: the storefront writes both its pages and what it tells search
engines from the shop's shipping methods and return terms (#606).

Why some steps are a member's word:

- **Supplier's text.** An imported product keeps no copy of the text it
  came with, so timada cannot tell an edited text from the supplier's.
- **Consent banner and contact.** They are the storefront's, which is the
  merchant's own application; timada holds no setting for them.
- **Sitemap and robots, and the sitemap in Search Console.** Reading them
  means fetching the storefront or asking Search Console's sitemaps list,
  which nothing does yet.
- **Core Web Vitals.** A new shop has no field data, and a lab run from
  the dashboard would need Google's PageSpeed service, its key and its
  limits. The step links to PageSpeed Insights for the shop's address.

## The target

Before the first campaign the merchant sets a monthly profit after ads, in
a currency, and a day to reach it by (`tenant:manage`). Each
`Launch::TargetSet` carries who set it; the time is the event's. A change
is another event: the page shows the target that stands and, under it, the
ones before.

Progress is the last thirty days' profit once Google Ads is paid, over the
target. It is known only when every sale of those days says what it left
(#546) and the shop has a Google Ads account chosen; a month in the red
reads as 0 %.

## The checkpoints

Four, counted in days from the moment the first campaign went live
(`Campaign::WentLive`, see [the first campaign](google.md#the-first-campaign-691)).

| Day | Reads | Go on | Fix | Stop or change niche |
|---|---|---|---|---|
| 21 | The first campaign's return on ad spend, as Google Ads counts it, over its first 21 days | 0.70 or more | 0.40 to 0.70, or under 0.40 on 1,000 clicks or fewer | under 0.40 with over 1,000 clicks |
| 45 | What the orders that came from a Google ad sold for 1 spent, over the 21 days before, against what those sales must bring to break even | at or above break-even | within 30 % of it; or lower with no tuning round made yet; or break-even not known | under half of it, after a tuning round |
| 90 | Search Console, over the four weeks before and the four before those | two categories in positions 1 to 20 | fewer, impressions rising; or Search Console not read | fewer, impressions not rising |
| 120 to 180 | What is left of the revenue once goods, costs and Google Ads are paid, every channel, over the thirty days before and the thirty before those | 15 % or more | 5 to 15 %; or under 5 % and improving; or not known | under 5 % and not improving |

The rules are pure code (`timada_launch::judge`). Every number in the
table is a shop setting with that default (`Launch::ThresholdsSet`); they
come from one case study and are expected to be tuned. A threshold changed
later does not rewrite a checkpoint read before.

### When a checkpoint is read

Nothing in the event store fires on a date. A checkpoint is read when the
daily read of Google Ads ends for the shop (`AdsChecked`) and its day has
come: one checkpoint a pass, the earliest owed first, so a node that was
away catches up over a few passes. Two nodes that read the same checkpoint
record it once (`Launch::CheckpointReached`, keyed by the checkpoint and
the campaign's start). A shop whose Google connection lapsed is not read
until it connects again.

The event keeps the numbers as they were that day, what they say and why.
A campaign started again in another Google Ads account has its checkpoints
read again, in place of the old readings.

### The notice

`CheckpointReached` owes the shop's owners a mail: the checkpoint, what its
numbers say, the numbers, and a link to the page. It goes through the
mailroom (`Kind::CheckpointNotice`) like the other platform notices: once a
checkpoint, retried for a week when the mail server fails.

### The decision

At each checkpoint read, a member with `tenant:manage` records what they
decided (go on, fix, stop) and why (`Launch::DecisionRecorded`). Said
again, the last word stands and both stay in the stream.

A "stop" is advice. Nothing is paused, unpublished or switched off by
timada because of a checkpoint or a decision.

## On the shop's home

Once the first campaign runs, a tile shows the next checkpoint and its
date, the last one's result, and how far the last thirty days are toward
the target.
