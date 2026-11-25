# Launch: target and checkpoints

A new shop loses money for weeks by design. Without a target fixed
beforehand and dates to read the numbers at, "is this working" is answered
by mood, and a niche that should be dropped is kept.

`/t/{slug}/checkpoints` (Sales, for `orders:read`) holds both. The domain
is `crates/launch` (`timada-launch`): one stream a shop, in the shop's own
tenant. What reads the numbers and mails the owners is
`dashboard/src/checkpoints.rs`.

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
