# Google

A shop reads three things from Google: where it ranks (Search Console), what its ads
cost (Google Ads) and whether its products are accepted (Merchant Center). Each needs
the consent of the Google account that owns them. A shop keeps **one connection** for
all three (#686): one place to say yes, one grant kept sealed, one "connect again".

Search Console is the first kind of access a shop can turn on. Google Ads and Merchant
Center are modelled (`timada_google::ACCESSES`) and join the page with what reads them
(#688, #687).

## Setting it up (operator, once)

1. In a Google Cloud project of the platform's, enable the **Google Search Console
   API**.
2. Under *APIs & Services › OAuth consent screen* (Google Auth Platform), set the
   application's name, support address, home page and privacy policy, choose
   **External** users, and add the scope
   `https://www.googleapis.com/auth/webmasters.readonly` under *Data Access*. That
   page is where Google says whether it counts the scope as sensitive.
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
  the shop's, in the **platform's streams**: a storefront's key reads the shop's
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

The browser suite runs against a stand-in (`tests/e2e/google.ts`), which
`config/e2e.toml` points the four addresses at.
