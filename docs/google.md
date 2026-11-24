# Google

A shop reads three things from Google: where it ranks (Search Console), what its ads
cost (Google Ads) and whether its products are accepted (Merchant Center). Each needs
the consent of the Google account that owns them. A shop keeps **one connection** for
all three (#686): one place to say yes, one grant kept sealed, one "connect again".

Search Console and Google Ads are the kinds of access a shop can turn on. Merchant
Center is modelled (`timada_google::ACCESSES`) and joins the page with what reads it
(#687).

## Setting it up (operator, once)

1. In a Google Cloud project of the platform's, enable the **Google Search Console
   API** and the **Google Ads API**. The project's access level at Google Ads sets how
   many questions a day it may ask (see "Google Ads" below): put it in
   `google.ads_calls_a_day`.
2. Under *APIs & Services › OAuth consent screen* (Google Auth Platform), set the
   application's name, support address, home page and privacy policy, choose
   **External** users, and add the scope
   `https://www.googleapis.com/auth/webmasters.readonly` under *Data Access*, and
   `https://www.googleapis.com/auth/adwords` for Google Ads. That page is where Google
   says whether it counts a scope as sensitive.
3. Under *Credentials*, create an OAuth client of type **Web application** with one
   authorized redirect URI: `<server.url>/google/callback`.
4. Put its id and secret in `[google]`, next to `[secrets] key`, which seals what the
   accounts grant:

   ```toml
   [google]
   client_id = "….apps.googleusercontent.com"
   client_secret = "…"   # TIMADA__GOOGLE__CLIENT_SECRET in production
   ```

Without the id and the secret, or without `secrets.key`, the server says so when it
starts and no shop connects a Google account.

## What Google asks of the application

Read on 2026-11-23 in Google's documentation; check again before opening it to
merchants.

- **Publishing status.** While the consent screen is in *Testing*, only the test users
  listed on it (100 at most) can say yes, and **a grant lapses seven days after
  consent**: every shop would show "connect again" each week. Publish the application
  (*In production*) before any merchant connects.
- **Verification.** An application that asks for a *sensitive* or *restricted* scope
  and is not verified shows Google's "unverified app" warning before the consent
  screen, and is limited to **100 new users over the whole life of the project**: the
  count is never reset. Verification of a sensitive scope asks for a home page and a
  privacy policy on a domain verified in Search Console, a justification for each
  scope and a video of the consent flow; Google gives about ten working days.
- **Which scopes.** Google's public pages do not say whether
  `webmasters.readonly` is sensitive: the Cloud console shows it when the scope is
  added (step 2). Google Ads (`adwords`) is **restricted**: verification then also
  asks for a security assessment by an assessor Google lists, every twelve months.
  Merchant Center (`content`) has to go through verification too; the page does not
  say at which of the two levels.
- **Brand.** Without brand verification the consent screen shows the application's
  domain rather than its name and logo.

Not confirmed: whether Search Console read access is sensitive, and how long
verification takes in practice.

## What a shop does

`/t/{slug}/google`, for members with `pages:read`; those with `pages:manage`:

1. **Connect.** The member is sent to Google with the kinds of access the shop turns
   on, and the account's e-mail address so the page can name it. Google always asks
   (`prompt=consent`), which is what makes it hand back a grant that can be renewed,
   and keeps what the account granted before (`include_granted_scopes`): turning one
   more kind of access on later replaces the grant with a wider one. An account may
   untick boxes on Google's page: the connection covers what it granted, and nothing
   at all is refused.
2. **Choose the site.** The page lists the Search Console sites the account reads,
   without the ones it added and never proved its own, and proposes the one at the
   shop's storefront address. Connecting again with the same account keeps the site;
   another account sees other sites, so the choice is made again.
3. **Disconnect.** The grant is forgotten here. It is *not* withdrawn at Google:
   Google withdraws per account and application, not per grant, so withdrawing one
   shop's would end every other shop's connected to the same account. The page links
   to the account's own settings for that.

## How it is kept

- The connection is an aggregate of its own, `timada-google/Connection`, whose id is
  made of the shop's (`connection_id`), in the **platform's streams**: a storefront's key reads the shop's
  streams and never sees the sealed grant.
- The grant's refresh token is sealed for the shop (`seal::google_sealer`, bound to
  the shop's id) before it is recorded. It does not lapse while it is used; Google
  drops one unused for six months.
- The access a grant renews lasts an hour and is **never recorded**: each node keeps
  the one it last got, by shop, and asks for another five minutes before it lapses
  (`Google::access`). Recording them would write every connected shop's stream
  twenty-four times a day for a value any node can ask for again.
- **Connect again.** Google refusing to renew a grant (`invalid_grant`: withdrawn by
  the account, or unused too long) is recorded on the connection as `Lapsed`, once, by
  whichever page or pass met it; an access turned down within its hour is asked for
  again first (`Google::read`). Everything that reads through the connection checks
  `ConnectionState::reads` and says "connect again" until the member does. Google not
  answering says nothing of the grant.

## Search Console (#683)

`/t/{slug}/search-console`, for members with `pages:read`: the site's clicks,
impressions, click rate and average position over the last 7, 28 or 90 days, then the
queries it showed for and the pages found, the most clicked first.

### The daily read

`dashboard/src/search_passes.rs`. Every node sweeps, every `google.search_check_secs`
(five minutes), the shops the read model says are due, each node those of its part of
the shops. The shop's own streams decide: its connection says whether Google can be
read and for which site, its watch (`timada-google/SearchWatch`) from when a pass is
due.

A pass asks Google, for web search and finished days only (`dataState: final`):

1. the site's numbers **by day** over the days it may be missing. One question, not
   grouped by query or page, which also tells which days are finished: Google answers
   with those only, two to three days behind, in Pacific time;
2. for each missing day, its most clicked **queries**, then its most clicked
   **pages**: two questions over one day each;
3. for each missing day, **which page showed for which query**: one question over
   that day, grouped by both, for 5,000 pairs of which the 1,000 most shown are kept.

That shape follows Google's limits, read on 2026-11-23:

- the counted quotas are far away: 1,200 questions a minute for a site and for an
  account, 30,000,000 a day for the application. A daily pass is four questions on a
  normal day;
- the limit that matters is the **load** quota, which Google does not put a number on:
  grouping or filtering by query or by page is costly, both at once "the most
  expensive", and a longer range costs more. So a pass asks for rows one day at a
  time, and groups by both once a day only, because nothing else tells which page
  answers which query (#684). Google lists rows by clicks, and a page on the second
  page of results has few or none: more pairs are asked for than kept, and the ones
  kept are the most shown;
- a site's totals are their own question because the rows do not add up to them: the
  queries too few people typed are withheld from the rows and counted in the totals,
  and a search that shows two pages of the site counts once for the site and once for
  each page.

A shop just connected is owed 90 days, fetched the newest first, ten a pass, with the
next pass ten minutes later while older days are left; after that, once a day.

### What is kept

- **A day** is one event on a stream of its own (`SearchDay`, id made of the shop, the
  site and the day), written once: Google does not change a finished day, and two
  nodes that both fetched it record it once. Its rows live in the event (the 500 most
  clicked queries, the 500 most clicked pages and the 1,000 most shown pairs of a
  page and a query), so a rebuilt read model and every node hold the same.
- **The read model** (`google_search_days`, `google_search_queries`,
  `google_search_pages`, `google_search_page_queries`) is keyed by shop, site and day. The site's numbers are kept
  486 days (sixteen months, as far back as Search Console goes), the queries, the
  pages and the pairs 92 days: each new day drops what is older, so nothing sweeps them. A shop that
  chooses another site keeps the earlier one's rows apart, to age out.
- A day with no search that showed the site is absent from Google's answer: it holds
  no row, and counts for nothing.

### When it fails

- **Google does not answer, or refuses**: the pass leaves `SearchCheckFailed` on the
  watch with the reason, which the page shows, and the next pass waits an hour, then
  two, up to a day. The days recorded before the failure stand.
- **The grant is no longer honoured**: recorded on the connection (`Lapsed`), not as a
  failed pass. The shop is not due again until its member connects again, and the
  page says "connect again" above what was read before.
- **The store fails**: nothing is recorded; the next sweep tries again.

### What a page is

`search_console/landing.rs` reads a page's address the way the storefront serves it:
a category at its path in the tree, a product under its category or at `/p/{slug}`,
an article at `/blog/{slug}`, one of the shop's own pages at `/{slug}`, each under
`/{language}` in a language other than the main one. It is told from what is live in
the back office now: an address that moved since, or one the storefront serves on its
own (the cart, a search), reads "Other". A page's or an article's slug in another
language is not among the back office's rows, so those are told by the main
language's.

## Categories in search (#684)

`/t/{slug}/search-console/categories`, the tab beside the overview, for members with
`pages:read`. Worked out on each visit from `google_search_page_queries`
(`search_console/ranking.rs`, pure), nothing of it is stored:

- **A category's best query** is the one its page was shown for the most over the
  last 28 days (four weeks ending on the last day held): the most shown, not the
  best placed, so a rare query at position 1 does not hide a category whose main
  query sits at 14. A category served in several languages is one category.
- **On the second page**: the categories whose best query averages a position that
  rounds to 11 to 20 (10.5 up to 20.5), with the query, its impressions and its
  position, both week by week over the last six weeks, and what the catalog says of
  the category: the products its page lists, whether it has a description, when it
  last changed. A line above the table counts the categories on the first page, the
  second, further, and not shown.
- **New shop check**: shown for 180 days from the first day Search Console holds for
  the site, due on day 90. Two answers: how many categories have their best query in
  positions 1 to 20 (two wanted), and whether the site's impressions over the last 28
  days are rising, flat or falling against the 28 before (a tenth either way). The
  first day is the first Google showed the site on, as far back as was read (90 days
  before the shop connected): Search Console's own "first indexed" date is not read.
- **Queries without a category**: what the shop showed for over the last 28 days
  through no category page, the 50 most shown, each with the page Google answered
  with the most. Candidates for a new category or for an article.

A day read before the pairs were (none on a released version) holds no pair.

The browser suite runs against a stand-in (`tests/e2e/google.ts`), which
`config/e2e.toml` points the four addresses at.

## Google Ads (#688)

`/t/{slug}/ads`, for members with `orders:read`: what the shop's Google Ads account
cost and brought over the last 7, 28 or 90 days (cost, impressions, clicks,
conversions, conversion value and return on ad spend), then the same by campaign, by
kind of device, by ad group, by category, by product and by search. Read only:
nothing is changed in Google Ads.

### The account

Once the connection covers Google Ads, `/t/{slug}/google` lists the Google Ads
accounts the grant reaches and the member chooses the shop's: the accounts the Google
account reaches by itself, and the clients of the manager accounts it reaches (then
read through the manager, named in `login-customer-id`). The choice keeps the
account's number, name, currency and time zone on the connection
(`Connection::AdsAccountChosen`). The same Google account connecting again keeps it;
another account, or a grant without Google Ads, drops it.

### What Google asks (read on 2026-11-24)

- **No developer token.** Google stopped reading the `developer-token` header on
  2026-09-09: the access level belongs to the **Cloud project** of the OAuth client.
  Nothing is sent, and there is nothing to configure but the level's limit.
- **Access levels**, in operations a day (a sliding 24 hours), on real accounts:

  | Level | A day | How it is obtained |
  |---|---|---|
  | Test | none (test accounts only) | on enabling the API |
  | Explorer | 2,880 | applied for, mostly automated |
  | Basic | 15,000 | brand verification first |
  | Standard | no limit | manual audit, about ten working days |

- **One question is one operation.** A report asked as a stream
  (`googleAds:searchStream`) counts once however many rows it carries, so every
  report is asked that way, over a range of days.
- **The API's version is in the address** (`google.ads_url`, `v26` today). Google
  retires a version about a year after it ships: moving to the next one is a change
  of that default, checked against the release notes.
- **The `adwords` scope is restricted** (see "What Google asks of the application").

Not confirmed against a real account: that `search_term_view` answers for Shopping
campaigns as it does for Search ones (Google's page says only that it leaves out
Performance Max), and that an item's id comes back in lower case (it is matched that
way whatever Google does).

### The daily read

`dashboard/src/ads_passes.rs`, shaped like the read of Search Console: every node
sweeps, every `google.ads_check_secs`, the shops the read model says are due, each
node its part; the connection and the watch (`timada-google/AdsWatch`) decide.

A pass asks **four questions**, each over the same range of days:

1. `campaign` by day and device: the totals, and the account's currency;
2. `ad_group` by day;
3. `shopping_performance_view` by day and item: the feed's products;
4. `search_term_view` by day, for the searches that brought a click.

The days are the **account's own** (its time zone), up to its yesterday. Google goes
on counting a day after it ends: a conversion is counted on the day of the click that
led to it. So every pass reads the last **14 days** again, and a day is recorded again
only when its numbers moved (`AdsDay::AdsDayFetched`, the last one standing). A shop
that just chose its account is owed **90 days**, read a month a pass, the newest
first, ten minutes apart.

**The allowance.** Each pass records how many questions it asked
(`AdsChecked.calls`), and the read model adds them up by day (UTC) for all shops and
all nodes. A sweep stops once the day's passes have used **four fifths** of
`google.ads_calls_a_day`: the shops left stay due and are read when there is room,
and the pages that list the accounts to choose from keep the rest. At "Explorer" that
is about 570 shops a day. Google's own refusal (`RESOURCE_EXHAUSTED`) is recorded
like any failure and tried again later. The tally is by calendar day where Google's
window slides, and a node counts what its read model has seen: both can let a few
questions through near the limit, which Google then refuses.

### What is kept

In the read model, keyed by shop, account and day, money in millionths of the
account's currency: the day's totals, campaigns by device and ad groups for 486 days;
products (2,000 a day at most, the ones that cost the most) and searches (1,000 a day)
for 92 days. An account changed leaves the other one's days to age out.

### What the page tells

The return on ad spend is **Google's**: the value of the conversions Google Ads
counted over what the clicks cost. It depends on what the Google tag reports (#537),
and is not the shop's own count of its orders: that is the sales report's (#689).

"By category" adds the products up by the categories the catalog files them under
**today**; a product in two categories counts in both. An item the catalog no longer
has reads "No longer in the catalog".
