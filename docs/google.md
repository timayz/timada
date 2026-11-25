# Google

A shop reads three things from Google: where it ranks (Search Console), what its ads
cost (Google Ads) and whether its products are accepted (Merchant Center). Each needs
the consent of the Google account that owns them. A shop keeps **one connection** for
all three (#686): one place to say yes, one grant kept sealed, one "connect again".

A shop turns on the kinds of access it wants read: Search Console, Google Ads,
Merchant Center.

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
kind of device, by ad group, by category, by product and by search. The page only
reads; what timada creates in Google Ads is [the first campaign](#the-first-campaign-691).

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

## The first campaign (#691)

A new shop's first campaign is always the same shape. timada builds it from
the shop's categories, and creates it in the shop's Google Ads account once
a member approved it. The page is `/t/{slug}/ads/campaigns`, reached from
the Google Ads page; the code is `dashboard/src/campaigns.rs`, the
aggregate `Campaign` in `crates/google`.

Two campaigns are offered:

| Kind | What Google Ads gets |
|---|---|
| `shopping` | A Standard Shopping campaign with manual bids, priority 0, one ad group a category, the same starting bid on each |
| `brand` | A Search campaign with manual bids on Google's own results, one ad group, the shop's name as phrase keywords, one responsive search ad that leads to the shop's address |

### What it waits for

The page lists what is missing and offers no form until nothing is. Each
check reads what timada already holds; none asks Google.

| Check | Read from |
|---|---|
| A Google Ads account is chosen | the shop's connection |
| The Google tag counts the sales | the tag has a Google Ads id and the label of its purchase conversion |
| Merchant Center accepts every product (`shopping`) | the last read of Merchant Center (`merchant_standing().all_accepted_for_ads()`) |
| The feed tells campaigns each product's category (`shopping`) | the feed's label for campaigns is chosen |
| A category holds products to advertise (`shopping`) | a published category with products, not kept out of ads |
| The launch path's steps before ads hold (`shopping`) | every step of [the launch path](launch.md#the-launch-path-696) marked "before ads" |
| The shop's address is known (`brand`) | where shoppers find the shop, on the API keys page |

"Counts the sales" is what timada can know. Whether Google Ads recorded a
purchase cannot be read before a first ad was clicked: Google Ads counts a
conversion only after an ad click.

### One ad group a category, through the feed's label

The feed's label for campaigns (one of `custom_label_0` to `4`, chosen on
the Shopping feed's page) says, on every product, the category it is filed
under first, as the category's id in small letters. On a product whose
main category is kept out of ads it says `no_ads` instead. It takes the
place of the merchant's own value on that label.

Each ad group shows the products whose label says its category, and
excludes every other product. So a product of a category kept out of ads,
or filed nowhere, is shown by no ad group. The id is used, not the name or
the address: renaming a category leaves its ad group showing its products.

An ad group is named after its category, with the categories above it
(`Chats > Arbres`), so no two share a name. A campaign is created with a
hundred ad groups at most.

### Settings and their defaults

| Setting | Default | Limits |
|---|---|---|
| Campaign name | `Shopping · {shop}` / `{shop} · name` | no other campaign of the account bears it |
| Daily budget | 10.00 / 2.00, in the account's currency | up to 10,000.00 |
| Starting bid | 0.30 | not above the daily budget |
| Countries | the ones the shipping methods name | 1 to 30, among those `geo_target_of` knows |
| Phones, tablets (`shopping`) | -55 %, -40 % | -90 to 900, or nothing |
| Starts | paused | paused or live |
| Ad groups (`shopping`) | every category worth advertising | a line each: the category's id, then the ad group's name |
| Searches, headlines, descriptions (`brand`) | the shop's name; three headlines; two descriptions, in English | 1 to 10 searches; 3 to 15 headlines of 30 characters; 2 to 4 descriptions of 90 |

A country is told to Google Ads by its number there, which is 2,000 plus
its ISO 3166-1 number (France is 2250). Google's list of geo targets
confirms it for every country checked; its documentation does not state
the rule.

### From the form to Google Ads

1. **Prepared.** The form's values become a [proposal](proposals.md) of
   kind `ads_campaign`, whose subject is the kind of campaign and whose
   fields are the settings. Nothing is sent to Google Ads. While it waits,
   the page links to it in place of the form.
2. **Approved.** A member with `proposals:approve` and `ads:manage` reads
   the whole of it, changes what they want and approves.
3. **Created.** The saga that applies proposals looks in the account for a
   campaign of the approved name. One that holds the ad groups asked for is
   this one, made by an attempt that did not get to record it, and is
   recorded. Otherwise the campaign is sent as one request, which Google
   Ads takes whole or not at all, and recorded on the shop
   (`CampaignCreated`). So an approval seen by two nodes ends with one
   campaign.
4. **Refused.** What Google Ads refuses (Merchant Center not linked to the
   account, a name taken by another campaign, billing) ends the proposal as
   not applied, with Google's words. Nothing was created; it is prepared
   again.

Creating a campaign costs as many operations of the platform's daily
allowance as it has parts: a dozen, plus five an ad group. It is not
counted in what the daily passes may use.

### Running, and the three weeks after

A campaign created paused is set running from the page (`LiveRequested`,
then the saga `timada-dashboard/ads-campaigns` tells Google Ads), or at
Google Ads itself: the daily read records the first day it showed an ad.
Either way `WentLive` is written once, with the moment. The
[checkpoints](launch.md) count their days from it, and the page says to leave the campaign
alone for its first 21 days.

A campaign's structure is created, never changed, from timada: its bids
are tuned by [rounds](#tuning-rounds-692), the rest by hand at Google Ads. A
shop that moves to another Google Ads account may create its campaigns
again there.

### Not confirmed against a real account

- That Explorer access may create campaigns: the documentation restricts
  account creation, users, planning and billing, and does not list
  campaigns.
- The shape of the listing groups when the ad group is created in the same
  request (a temporary number for both), and the "everything else" node of
  a custom label keeping its index.
- That a duplicate campaign name is refused on creation as it is on a
  rename (`DUPLICATE_CAMPAIGN_NAME`).
- That labels compare whatever their capitals: the feed and the campaign
  both write the category's id in small letters, so nothing depends on it.
- Device adjustments on a Standard Shopping campaign with manual bids, sent
  when the campaign is created.

## Tuning rounds (#692)

After its first three weeks a manual campaign is tuned by the same few
rules every round. `/t/{slug}/ads/tuning` (from the campaigns page) asks
for a round, says what the last one did and holds the thresholds. The rules
are pure code in `crates/google/src/tuning.rs` (`tune`); the saga, and what
applies an approved change, in `dashboard/src/tuning.rs`.

A round reads the first campaign's last days from the read model, and what
the shop sold over the same days from its orders. It asks Google nothing: a
bid is the one timada last set. Each change it finds becomes one
[proposal](proposals.md), with the numbers it was decided on as its reason.
Nothing is sent to Google Ads until a member approves.

| What | Rule (defaults) | Proposal |
|---|---|---|
| A category's bid | With 100 clicks: one step (0.02) up when the return is at or above its target, two when half again above; one step down under half the target, two under a quarter. Barely serving (under 200 impressions): one step up. | `ads_bid` about `group:{ad group}`, field `max_cpc` |
| A draining product | 150 clicks, no conversion at Google Ads and none sold by the shop's orders | `ads_exclusion` about the product, field `ads`: `shown` to `kept out` |
| A product below break-even | It sells, under its break-even | none: flagged on the page, for its price |
| A winner | 50 clicks and a return half again above its break-even | `ads_campaign` about `winners`: a Shopping campaign of its own, priority 2, two steps above the starting bid. Proposed once; later winners are flagged |
| A search | 3 clicks, no conversion, no word in common with the ad group's category or the products it showed | `ads_negative_term` about the search, field `search`: `shown` to `excluded` |
| A device | With 100 clicks: 5 points more or less when its return is a quarter off the campaign's, 10 when half off | `ads_bid` about `device:{device}`, field `adjustment` |

The target of an ad group is its category's break-even when the shop's
sales say it (#689), and the shop's target return (3.00) otherwise. With
too few clicks nothing is proposed, and the round says so under "Left
alone". A bid or an adjustment just changed is left to show for 14 days
before another round looks at it. A subject that already has a proposal
waiting gets no second one.

Every threshold is the shop's to set on the page (`Tuning::TuningSet`).
"A round every so many days" makes rounds come by themselves: when the
daily read of Google Ads ends and one is due, the saga asks for it, under
the name `timada-tuning`, and the members who may approve are mailed that
proposals wait. A round is refused while the campaign does not run or is in
its first 21 days.

### What an approval does

- **A bid** is read back from Google Ads first. One that is neither what
  the proposal was prepared against nor what it would set was changed by
  hand since: it is left alone and the proposal says what Google Ads holds.
  Otherwise the ad group and its product group are set together, and the
  campaign records it (`BidSet`).
- **A device's adjustment** goes the same way, on the campaign's criterion
  for the device (`AdjustmentSet`).
- **A search** becomes a negative keyword of the campaign, typed exactly
  so (`TermExcluded`).
- **A product** is marked "kept out of ads" in the catalog
  (`Product::AdsExcluded`, also a checkbox on its Shopping feed tab). The
  feed then says `no_ads` on its label for campaigns, so no ad group shows
  it once Merchant Center read the feed again. It is never removed from
  the shop, and nothing is sent to Google Ads.

Not built here, as the issue leaves them for later rounds: the move to a
target return per margin tier, then to profit as the conversion value.

Not confirmed against a real account: that a campaign's device criteria
are there to update before an adjustment was ever set (a missing one is
created), and that excluding the same search twice is refused rather than
doubled (timada records an exclusion and does not send it again).

## Profit after ads (#689)

The sales report (`/t/{slug}/sales`) sets what Google Ads cost against what the shop's
own orders say, on the same days (`dashboard/src/sales_report.rs`):

- **For the shop**: ad spend and profit after ads, beside the period's totals.
- **Paid and free**: the orders by the kind of click that brought the shopper last.
  An order is matched by the source it kept (#538): `google / cpc` (a `gclid`, or a
  campaign tagged with a paid medium) is Google Ads; a paid medium from anywhere else
  is "other paid clicks", whose spend is not read; everything else came without a paid
  click, and its share of the revenue is told. A second channel that tells its spend
  is one more row.
- **By product and by category**: what Google Ads cost for each (from the products
  the daily read keeps), what the orders Google Ads brought bought of it, the return
  (sales for 1 spent), the **break-even** and the gap between the two. A return above
  the break-even earns, one below costs.

The break-even is what has to be sold for 1 left before ads, from the period's own
sales of the product or the category: the lines that kept their costs (#546). A
product with ads paid for and nothing sold costs, whatever its break-even.

What to keep in mind when reading it:

- The spend is in the **account's currency**; sales in another currency are left out
  of the comparison.
- The days are the same calendar days on both sides: the shop's clock for the orders,
  the account's for the spend. Two different time zones shift a day's edges by their
  difference.
- The total spend covers every campaign; the products' rows only what Google Ads
  tells by product (a Search campaign on the shop's name has none).
- This is the shop's count. Google's own return, from the conversions its tag
  reports, is on `/t/{slug}/ads`.

The same numbers leave as a CSV file from the page, and on a schedule like the other
exports (#625): a row a currency for the totals, then each source, each kind of
click, each product and each category, told apart by their first cell.

## Merchant Center (#687)

`/t/{slug}/merchant-center`, for members with `pages:read`: how many of the shop's
products Google accepts, still looks at or refuses, for Shopping ads and for the free
listings; what Google holds against the account; and the products it does not simply
accept, each with Google's reason, the field of the feed it names and a link to the
product. A product's own page says where it stands. Read only: nothing is changed in
Merchant Center.

### Setting it up (operator, once more)

Read on 2026-11-24. Besides enabling the **Merchant API** in the Cloud project and
adding the scope `https://www.googleapis.com/auth/content` to the consent screen:

- **The Cloud project has to be registered with a Merchant Center account**, once,
  before any call works. It is registered with **one** account, the operator's own (a
  standalone account with a verified site), not with each merchant's:

  ```sh
  curl -X POST -H "Authorization: Bearer $ACCESS" -H "Content-Type: application/json" \
    "https://merchantapi.googleapis.com/accounts/v1/accounts/$ACCOUNT/developerRegistration:registerGcp" \
    -d '{"developerEmail": "you@example.com"}'
  ```

  The caller has to be an administrator of that account, and `developerEmail` a
  Google account, not a service account. Until it is done Google answers `401`,
  "GCP project … is not registered with the merchant account": the Google account
  page shows that sentence where it would list the accounts, and nothing is recorded
  as a grant to renew.
- **The Content API for Shopping is gone** (sunset on 2026-08-18): everything here
  goes through the Merchant API, `v1`.
- **Quotas are set by Google per account**, from how many products it holds: listing
  the products is allowed about twice a full listing a day. A daily read and a few
  asked for fit; `google.merchant_ask_again_secs` (five minutes) is how long a member
  waits before asking again.

### The account

Once the connection covers Merchant Center, `/t/{slug}/google` lists the Merchant
Center accounts the grant reaches and the member chooses the one that takes the shop's
feed (`Connection::MerchantAccountChosen`). Sub-accounts of an advanced account are
not listed: connect a Google account that reaches the shop's own account.

### The read

`dashboard/src/merchant_passes.rs`, once a day per shop like the other two, and when a
member with `pages:manage` presses "Read now". A read lists every product of the
account, a thousand a question, then asks what is held against the account.

A product's standing in a place (Shopping ads, the free listings) is **refused** as
soon as one country refuses it, **pending** while one has not answered, **accepted**
otherwise, and "not sent" when Google does not name the place for it. Issues of other
places (display ads, local listings) are left out.

What moved since the read before is recorded on the shop's feed
(`MerchantFeed::MerchantRead`): every product Google does not simply accept, then the
accepted ones up to 5,000 in all (the rest are counted). A read that says the same is
not recorded again.

### What turns for the worse

The read's event says what turned since the one before: the products refused that
were not, and the issues the account did not have. A first read turns nothing, and a
product refused from its first appearance is not news. The shop's **owners** are
mailed once a day at most (`dashboard/src/merchant_notices.rs`, through the mailroom
like the other notices), with a link to the page; what was put right before the mail
left is not told.

### For the launch path

`merchant_standing(…).all_accepted_for_ads()` is what "all products accepted" reads:
every product Merchant Center holds accepted for Shopping ads, none pending, refused
or not sent there.

Not confirmed against a real account: the exact wording and status of the
"not registered" answer, and that `offerId` comes back in the case the feed gave it
(the page matches it to the catalog whatever its case; the product's own page looks
it up as the feed writes it).
