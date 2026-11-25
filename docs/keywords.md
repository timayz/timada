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

A second call asks who holds the first results of a search: the first two pages
Google gives for one keyword from one country in one language, on a desktop
(DataForSEO SERP, `serp/google/organic/live/regular`, twenty results asked for). Only
the organic results are kept, twenty at the most, each with its place, its site, its
address and its title; ads and Google's own boxes are passed over.

Two pages because one says too little. Google's first page is mostly its own boxes (an
AI overview, shops nearby, products, images): for `arbre à chat` on 2026-11-25 it held
three to nine organic results, not the same from one search to the next. Twenty asked
for gave 13 organic results and thirty gave 22.

| | |
|---|---|
| a search of two pages, answered at once (the one made) | 0.004 USD |
| the same in DataForSEO's queue, answered in about five minutes | 0.0012 USD |
| in the queue's priority lane, about a minute | 0.0024 USD |

Prices read on 2026-11-23 for a page of ten results, and a search is charged by the
page: 0.004 USD for twenty results and 0.006 for thirty, both seen charged on
2026-11-25. The queue costs a third but
answers later through a second call; the search is made live so the pass that asks
records the answer, as a seed's fetch does. Moving to the queue is a change to
`DataforseoClient::first_results` and to the saga that waits for it.

Since November 2026 DataForSEO answers every organic result under Google's own
address, for good: `domain` is `google.com` and `url` an opaque `google.com/goto` link, on the
advanced endpoint as on the regular one (#791). The site is then read from the start
of the result's `breadcrumb` (`https://www.jardiland.com › Conseil & idées`), and so
is its address, down to the site only. A result whose breadcrumb names no site, a
video's for one, is passed over. An answer that names the site in `domain` is read
as before.

| | |
|---|---|
| a search, answered at once (the one made) | 0.002 USD |
| the same in DataForSEO's queue, answered in about five minutes | 0.0006 USD |
| in the queue's priority lane, about a minute | 0.0012 USD |

Prices read on 2026-11-23, for a page of ten results. The queue costs a third but
answers later through a second call; the search is made live so the pass that asks
records the answer, as a seed's fetch does. Moving to the queue is a change to
`DataforseoClient::first_results` and to the saga that waits for it.

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

What each study spent is kept on it (`cost_usd_micros`, summed from its fetches and
its searches).

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
held for it at the most it may cost (0.132 USD for a seed's keywords, 0.004 for a
search); a call that no longer fits under the
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
study fetched and from what its owner says of it. A study is scored again whenever one
of the two changes: a fetch lands, the owner saves their numbers, drops a keyword or
brings one back. No fetch is made for it.

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
currency. The score converts with what one unit of the currency is worth in dollars,
from the euro reference rates timada already reads for invoices; a price in dollars
needs none. The rate is the day's when the owner saves their numbers, and is kept with
them on the study, so every node scores it alike and a rebuilt read model gives the
same verdict; saving again takes the new day's rate. When the rates could not be read
the two are compared as they are, and the page says so until the owner saves again.

**The tests and the verdict.**

| test | passes when |
|---|---|
| main keywords | 3 or more |
| long tail | 10 or more |
| margin | a sale leaves more than zero |
| price / CPC | at or above what the sale needs |
| supplier | 200 products or more, entered by hand |
| competition | no specialist shop in the first results of 2 main keywords or more (see *Who holds the first results*) |

A test whose numbers are not there yet is *not checked*: nothing fetched, no price, no
cost per click on any kept keyword, a figure not entered, a site not marked. The verdict is **go** when
every test passes, **no-go** when one fails, **incomplete** otherwise.

**Settings.** The highest difficulty kept, how many main and long-tail keywords are
enough, the safety and the conversion rate are settings with these defaults
(`Thresholds`); nothing sets them yet, so every study is scored with the defaults. The 35 is Semrush's figure, taken because DataForSEO's score runs over
the same 0 to 100; it is a placeholder until the comparison below is made.

## Who holds the first results

A niche whose first results are marketplaces and department stores can be taken by a
specialist shop; one a specialist already holds is hard. The study's page answers it
under *Who holds the first results*.

**The searches.** *Search the first results* asks for the first two pages of each main
keyword, the 50 most searched at the most, in the study's country and language. The
button says how many searches that is and the most they cost before it is pressed.
Each is one paid call made in the background, a few seconds each, one after the other,
by a saga of its own (`first results`) so that a study's searches do not keep another
study's keywords waiting. They go through the day's caps as a fetch does, held at
0.004 USD each; a search past a cap or one the source failed says why on the page and
is asked again with the same button, which only asks for the keywords still without
their results. Once every main keyword has its own, *Search them all again* pays for
all of them anew. A keyword that stops being a main one keeps its results: brought
back, it is not paid for twice. There is no reuse across studies.

**The sites.** The page lists the sites found: for how many of the main keywords each
appears and at what places, the ones found for the most keywords first. A site is its
host without `www.`; a subdomain is a site of its own. A site with two results for one
keyword counts once, at its best place.

**The marks.** The owner marks a site as a *specialist* or a *generalist*. A mark
belongs to the user, not to the study (`DomainMarks`, one stream per user): Amazon
marked once is known on every study of theirs, and a mark changed changes them all.
2,000 sites at the most.

**The test.**

- it **fails** as soon as one specialist is found for 2 main keywords or more,
  whatever is left to search or to mark;
- it **passes** when every main keyword looked at has its results and every site found
  for 2 of them or more is marked, none a specialist;
- it is **not checked** otherwise.

A site found for one main keyword decides nothing, so it needs no mark: a study of
thirty keywords asks for a handful of marks, not three hundred. The 2 is `SEVERAL_KEYWORDS`.

The results of each search (`research_checks`), the marks (`research_domain_marks`) and
what they come to for a study (`research_domains`, and the competition on
`research_studies`) are kept by the read model, worked out again whenever the main
keywords, a search or a mark changes. The page reads the marks from the user's own
stream, so a mark shows at once.

## The report

A study's page shows, under its seeds:

- **the verdict** and the six tests, each passed, failed or not checked, with the
  numbers behind it;
- **one sale**: the price line by line down to what it leaves, the break-even return on
  ad spend, the price / CPC it needs and the highest cost per click that allows, then
  the cost per click of the kept keywords averaged by volume and the study's own
  price / CPC;
- **what the score reads**, in one form: the currency, the price and the costs, the
  brands to drop and the words to exclude (one a line, 200 of each at most) and how
  many good products were found at the supplier. A field left empty is nothing said:
  the supplier test stays *not checked* until filled. Saving scores the study again
  from the keywords it holds;
- **who holds the first results** of the main keywords, with the sites to mark;
- **the keywords**, the most searched first, by list: all of them, the main ones, the
  long tail, the spellings that count under another, then the dropped ones by reason.
  Each row has its volume, difficulty, cost per click (the source's, in dollars), the
  sale's price over it, *ads* or *SEO only* for a kept keyword, and a button: *Drop* on
  a kept keyword, *Bring back* on a dropped one, whatever dropped it. 2,000 keywords at
  most are dropped by hand, and as many brought back; past that, exclude a word.

A keyword two seeds gave is scored once, on its most searched row; the other row reads
*under another seed* in the list of all and is in no other list.

*Research* lists a user's studies **side by side**: verdict, main and long-tail counts,
searches a month over the kept keywords, the break-even, the price / CPC and what is
needed.

What each keyword became (`research_keywords.outcome`) and what they come to with the
verdict (`research_studies`) are kept by the read model, a moment behind the study: the
lists and the comparison read them off an index. The page runs the tests again over
what the owner says now, so a save shows at once while the keyword lists catch up.
Scoring reads every keyword of the study (20,000 at the most) and rewrites the rows
whose outcome changed. A study scored before this existed is scored at its next change.
The thresholds are the defaults below; nothing sets them yet.

### No export

A study's keywords are read on its page and nowhere else: there is no file to download.
A CSV export of the keywords with their score existed (#680) and was removed on
2026-11-25, before DataForSEO's binding terms were read in full (#774); it is not
wanted back, so the terms were not read further. What was read
on 2026-11-23: their Terms of Service (updated 12 June 2026) say nothing of exports,
and their own answers to legal questions, which are not part of the terms, allow a
"CSV export of derived analysis" and ask for a separate agreement for "onward
distribution of our datasets as an untransformed standalone feed, database dump,
public endpoint, or raw API service". No export, and no address that serves a study's
keywords without a session, is to be added without reading the terms again.

## When a call fails

A failure is one of two kinds, and what asks decides what to do with each:

- **unavailable**: no answer within 30 seconds, DataForSEO's own failure, or asked too
  often. Asking again later may do;
- **refused**: the login is wrong, the balance is spent, the account's own cost limit is
  reached, the country or the language is not held. It would be said again, so the
  reason is kept in DataForSEO's words for whoever reads it.

## Confirmed against the real service

Read on 2026-11-25 with the platform's login, for `arbre à chat` in France in French:

- a search takes the country code the keywords do: sent `location_code` 2250, it was
  made with `gl=FR` and `hl=fr` and answered with French shops;
- France in French returns a difficulty, for 616 of the 1,000 keywords of the call.
  The 384 others have none and are not held to be difficult.

## Not confirmed yet

- how DataForSEO's difficulty compares with the Ahrefs and Semrush scores the usual
  thresholds are written for. Fetch a few keywords whose scores in those tools are
  known and record the comparison here. The score's default of 35 for the highest
  difficulty kept waits on it. A first sign that its scale runs lower: `arbre à chat`,
  searched 18,100 times a month, has a difficulty of 5, and most of the fifty keywords
  searched the most have 0. The 35 is kept as it is for now (decided on 2026-11-25);
- that one search says who holds a keyword: three searches of `arbre à chat` within
  two minutes gave three different first pages, of three to nine organic results. Two pages
  are read since, which says more; whether it says enough is not measured.

Both need a live call, which the `keywords` command makes:

```sh
timada-dashboard keywords "arbre à chat" --country FR --language fr
timada-dashboard keywords "arbre à chat" --country FR --language fr --results
```
