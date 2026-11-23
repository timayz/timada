# Keyword data

A shop that lives on Google Shopping and search starts by choosing a niche from keyword
data, builds its categories from the same keywords, then follows where they rank
(#685). The numbers come from a **keyword source**: for a keyword, how often it is
searched a month, what a click on it costs advertisers, how hard the first page is to
reach (0 to 100) and what the searcher is after.

The one source is [DataForSEO](https://dataforseo.com), under the platform's own
account. The code that asks sits behind the `KeywordSource` trait
(`dashboard/src/keywords`), so nothing else names the vendor.

## Setting it up (operator, once)

1. Open a DataForSEO account and top it up: it is pay as you go, 50 USD at the least,
   with 1 USD of credit to try it.
2. Put the API login and password (the account's *API Access* page; the password is
   not the one the account signs in with) in `[dataforseo]`:

   ```toml
   [dataforseo]
   login = "ops@example.com"
   password = "…"   # TIMADA__DATAFORSEO__PASSWORD in production
   ```

3. Check it with one call, which prints the keywords and what the call cost:

   ```sh
   timada-dashboard keywords "arbre à chat" --country FR --language fr
   ```

   Without a seed the command lists the countries and languages the source has
   keywords for, which is free.

Without the login keyword research is off: the server says so when it starts and
nothing calls DataForSEO.

## What a call reads and costs

One call asks for the keywords that contain a seed's words, in any order, as they are
searched from one country in one language (DataForSEO Labs, Google,
`keyword_suggestions/live`). It returns the seed itself and up to 1,000 keywords, the
most searched first.

| | |
|---|---|
| a call | 0.012 USD |
| a keyword returned | 0.00012 USD |
| a call that returns 1,000 | about 0.13 USD |

Prices read on 2026-11-18. The code does not reckon what a call cost: it reads the
`cost` field of each answer and returns it with the rows, in millionths of a dollar,
which is what the caps count. The prices are only used for the most a call not made
yet may cost (0.132 USD), the room a cap keeps for it; see *What a day may cost*.

The cost per click is in US dollars whatever the country. A figure DataForSEO has
nothing for stays empty; it is never shown as zero.

The list of countries is read once a day and kept in memory. A country or a language
that is not on it is refused without a call.

## Studies

A signed-in user keeps the keywords of a candidate niche in a **study**, under
*Research* in the workspace: a name, up to 20 seed keywords, and the country and
language they are searched in, picked from the list above. A study is its owner's
only.

Nothing is fetched when a study is started. On its page the owner asks for the
keywords of one seed or of every seed that still wants its own; each seed is one
call, made in the background within a few seconds, and the page shows the fetch on
its way, then its rows. A seed can be fetched again later: its new rows replace its
earlier ones. The keywords read back per study, the most searched first, each with its
volume, cost per click, difficulty, intent, the seed it came from and the day it was
fetched.

A fetch that fails says why on the seed: a refusal in DataForSEO's words, or that the
source did not answer. The owner asks again. A fetch whose node stopped in the middle
shows "No answer" after ten minutes and can be asked again too; the rows of that one
call, if it was made, are lost.

What each study spent is kept on it (`cost_usd_micros`, summed from its fetches).

## What a day may cost

Every call is paid by the platform's account, whoever asks. `[research]` bounds it:

```toml
[research]
daily_usd_per_user = 1.0   # one user's calls, a day
daily_usd = 10.0           # every user's calls together, a day
reuse_days = 30            # a seed's keywords answer the next fetch of it
```

**A seed is paid for once.** A fetch first looks for a paid fetch of the same seed,
country and language made within `reuse_days`, in any study, by the same user or
another, deleted since or not. When there is one its rows are recorded on the study
that asks, with the day they were fetched, and nothing is paid or counted. So a seed
fetched within that time has no *Fetch again* button. `reuse_days = 0` pays for every
fetch. Two users who ask for a new seed in the same second may both pay for it: the
second does not know of the first yet.

**The caps.** A day runs from midnight to midnight UTC. Before a call is made, room is
held for it at the most it may cost (0.132 USD); a call that no longer fits under the
user's cap or the platform's is not made, and the seed fails saying which cap was
reached and when it frees. Once the source answered, what it says the call cost takes
the place of what was held. Three things follow:

- a cap is never passed, on one node or on several: the day's spending is one stream
  (`SpendDay`, id `2026-11-23`), every node writes it against the version it read, and
  two nodes cannot both take the last room;
- the last calls before a cap are refused while a little of it is left (less than
  0.132 USD), and a cap under 0.132 lets no call through;
- a call the source failed costs nothing and gives its room back. A node that stopped
  between holding room and charging keeps it held for ten minutes.

The study's page shows what the user's fetches cost today beside their cap, and a
warning once the next call would be refused. A day holds room for 20,000 calls at
most, whatever the caps: past it the platform's cap reads as reached.

**What was spent.** *Research spend*, under *Platform* for administrators
(`/admin/research`), lists the day's and the month's spend by user, with the number of
calls, beside the caps. It reads a table kept from the charges, a moment behind them.

## How a study is scored

A list of a thousand keywords does not say whether a niche is worth a shop. The tests
that decide are arithmetic (`crates/research/src/score.rs`), worked out from the rows a
study fetched and from what its owner says of it. Nothing of it is stored: a study is
scored again whenever one of its numbers changes, without a fetch. The page that shows
the score and edits its numbers is #680.

**Which keywords count.** A keyword is kept or dropped, with the first reason that
drops it, in this order:

| reason | when |
|---|---|
| excluded | dropped by hand, or it holds a word the owner excluded |
| brand | it holds a brand the owner listed |
| informational | the source says so, or it holds a question word |
| too difficult | its difficulty is above the highest kept (35) |
| volume too low | fewer than 150 searches a month, or no volume |

A brand or an excluded word is matched as whole words, whatever the case and the
accents: `nike` drops `chaussures nike`, not `nikel`. Question words are listed for
French and English; another language is left to the source's intent. A keyword without
a difficulty is not held to be difficult. A keyword the owner brings back is kept
whatever the filters say.

Two kept keywords that differ only by accents, hyphens or a plural `s` or `x`
(`arbres à chat`, `arbre a chats`) are one search: they count once, under the one
searched more, and its volume alone is counted.

A kept keyword is **main** from 1,000 searches a month and **long tail** below.

**One typical sale.** From the study's own numbers: the average price with VAT, the
supplier's cost, shipping, import charges, the VAT rate, the share of fixed costs, the
payment fee. The margin and the break-even return on ad spend are worked out by the
catalog's margin arithmetic, the one a shop's products use. Until a price and a
supplier cost are both said, nothing is.

**What a click may cost.** An ad pays when the price over the cost per click is at
least

    (break-even + safety) / conversion rate

with a safety of 0.4 and a conversion rate of 1.5 % unless set. A sale at 60.00 that
leaves 23.85 breaks even at 2.52, needs a price / CPC of 194.67, and so bears a click
of 0.308 at the most. Each kept keyword is *ads* or *SEO only* by its own cost per
click; the study as a whole is judged on the cost per click averaged by volume. A sale
that leaves nothing pays for no ad: every keyword is SEO only.

**Dollars.** The cost per click is in US dollars and the price in the study's
currency. The score converts: whoever asks gives what one unit of the currency is
worth in dollars (the euro reference rates timada already reads for invoices), and a
price in dollars needs none. Without a rate the two are compared as they are, the
score says so (`cpc_converted`), and the page must.

**The tests and the verdict.**

| test | passes when |
|---|---|
| main keywords | 3 or more |
| long tail | 10 or more |
| margin | a sale leaves more than zero |
| price / CPC | at or above what the sale needs |
| supplier | 200 products or more, entered by hand |
| competition | no specialist shop in the first results, entered by hand |

A test whose numbers are not there yet is *not checked*: nothing fetched, no price, no
cost per click on any kept keyword, a figure not entered. The verdict is **go** when
every test passes, **no-go** when one fails, **incomplete** otherwise.

**Settings.** The highest difficulty kept, how many main and long-tail keywords are
enough, the safety and the conversion rate are settings with these defaults
(`Thresholds`). The 35 is Semrush's figure, taken because DataForSEO's score runs over
the same 0 to 100; it is a placeholder until the comparison below is made.

## When a call fails

A failure is one of two kinds, and what asks decides what to do with each:

- **unavailable**: no answer within 30 seconds, DataForSEO's own failure, or asked too
  often. Asking again later may do;
- **refused**: the login is wrong, the balance is spent, the account's own cost limit is
  reached, the country or the language is not held. It would be said again, so the
  reason is kept in DataForSEO's words for whoever reads it.

## Not confirmed yet

Both need a live call, which the `keywords` command makes:

- that France in French returns a difficulty. The last line of the command's output
  counts the rows that have one;
- how DataForSEO's difficulty compares with the Ahrefs and Semrush scores the usual
  thresholds are written for. Fetch a few keywords whose scores in those tools are
  known and record the comparison here. The score's default of 35 for the highest
  difficulty kept waits on it.
