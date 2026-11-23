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

Prices read on 2026-11-18. The code does not reckon them: it reads the `cost` field of
each answer and returns it with the rows, in millionths of a dollar, for a cap to count.

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
Nothing bounds it yet: the caps per user and for the platform are #678.

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
  known and record the comparison here.
