# Mail

The dashboard sends two kinds of mail.

- **Platform mail**: sign-up, sign-in and password-reset codes, and invitations to
  join a tenant. It always goes through the `[mail]` server, from `mail.from`.
- **Shop mail**: what a shop sends its shoppers, meaning order confirmations, payment
  receipts and cancellations, review requests, storefront sign-in codes and newsletter
  confirmations. It goes through the
  server the merchant picked for shop mails, and through the `[mail]` server when none
  is picked. The sender's name is the shop's either way.
- **Marketing**: the shop's newsletter campaigns, and the test mail a merchant sends
  themself while writing one. They go through the server the merchant picked for
  marketing only, never the `[mail]` server: a shop with none picked cannot send a
  campaign (the send page and the test refuse with `mail_server_required`), and one
  sent before its server was deleted waits, with nobody claimed, until a server is
  picked again.

- **Automated mails** of the shop's automation scenarios (see
  `docs/automations.md`) are marketing too: the server picked for marketing only,
  each with a link that stops them.

## A shop's own servers

Members with `mail:manage` set them up on `/t/{slug}/mail`, up to ten per shop, so
that, say, order mails and newsletters go out through different providers. The form
takes:

- an optional name to tell the servers apart (without one, a server goes by its
  address);
- the host and port;
- the security: STARTTLS, implicit TLS or none;
- an optional user name and password;
- the address mails come from, plus an optional display name (the shop's name by
  default).

Saving sends a test mail through the new settings to the address given in the form,
which is prefilled with the member's own. The settings are recorded only if that mail
goes out. Every send through a shop's server, the test included, gives up after 20
seconds: a server that takes the connection and never answers, typically a port that
speaks TLS from the first byte (465) set up with STARTTLS, would otherwise hold it for
ever. The test then says to check that the security matches the port. Each server has its own page, where it is changed the same way (through a
test mail) or deleted. Leaving the password blank keeps the saved one, as long as both
the host and the user name stay the same: a saved password is never sent to another
server.

"Which server sends what" picks, among the shop's servers:

- the one shop mails go through, or the platform's server;
- the one marketing goes through, or none, and then no marketing goes out.

The first server a shop adds is picked for both; a later one sends nothing until
picked. Deleting a server leaves what it was picked for without one: shop mails then
go out through the platform's server, and marketing waits. A server set up before shops
could have several is listed as their first, picked for both.

A server switched for marketing while a campaign goes out sends its next mails. A
shop whose server stops accepting mail does not fall back to another one, nor to the
platform's server. The send fails and is recorded, so a broken setup shows instead of
being hidden: the codes, invitations, confirmations, order mails and message notices
are retried by the mailroom (see [Sending in the background](#sending-in-the-background)),
which gives up after about a day; the review requests, newsletters and hold reminders
are claimed before they are sent, so one that fails is logged and not retried. A
campaign's mails stop at the first failure the server is to blame for, until the
next look.

## Sending in the background

Sign-in codes (the platform's and the shops'), invitations, newsletter
confirmations, the shops' mails about their orders and the notices of a message's
arrival are mailed by the dashboard's sagas, not by the request that asked for them:
the command records the intent on the stream, a subscription on every node of the
region sees it, and the node that first writes a claim on the stream sends (an order
mail or a message notice has a stream of its own for that, `OrderMail` per order and
mail, `MessageNotification` per message, opened by the first claim). The
send itself happens off the subscription's cursor, in the node's **mailroom**
(`dashboard/src/mailroom`): the subscription only writes what is owed into the
node's `owed_mails` table and acknowledges the event; the mailroom works the table
right away and every five seconds, grouping the mails by shop and sending each shop's
one after the other, on a lane of its own, while up to sixteen shops go at once. The
first send a shop's server fails ends that shop's lane until the retry is due, so a
down server is probed once, not once per mail. One shop whose server takes twenty
seconds to not answer costs the others nothing, and a pass of a subscription never
fails for a mail server.

How a send went is recorded on the stream (`timada_core::Sending`):

- **sent**: handed to the server;
- **undeliverable**: the server refused the address for good (`550`, `551`, `553`, or
  `501` about the recipient), or it does not parse. The person corrects the address
  and asks again at once;
- **deferred**: anything else (no answer, a `4xx`, bad credentials). The next try may be
  made after 30 s, then 1, 2, 5 and 15 minutes, then 1, 4 and 12 hours, by whichever
  node looks once it is due;
- **failed**: the try after the last wait failed too, about eighteen hours in, or the
  next try would come after the intent itself expires (a sign-in code lives ten
  minutes, so its fifth failure is its last; an order mail or a message notice is
  worth sending for a day): the mail is given up on. An invitation or a confirmation
  stays open for days. A node whose lease lapsed while it was still trying records no
  failure over the claim another node took since.

`/admin/cluster` counts the mails this node still owes and lists the ones given up on
for a week, with how many tries were made (as the mail's stream counts them: each
`MailDeferred` and `MailFailed` carries the count) and the last failure's reason. The
members page shows an invitation that is being retried or was given up on, so the
inviter can invite again; an order's page shows each of its mails the same way.

An order mail the platform's server would send is checked against the payment
provider before it is claimed: an order whose payment the provider does not know as
the shop's is passed over (a `store:write` key could forge one), and a provider that
does not answer is asked again at the next look. A payment mail of an order settled
outside timada is passed over too.

## A shop's own wording

Members with `mail:manage` can also reword the shop mails, from "Mails to shoppers" on
`/t/{slug}/mail`. Each mail has an editor per language the shop mails in
(see [Several languages](#several-languages)), with:

- the subject and the whole HTML document;
- an optional plain-text version, for mail apps that show no HTML. Left empty, it is
  derived from the HTML;
- a preview with made-up data, plain text included;
- "Send me a test", which mails the draft to the member through the server picked for
  shop mails;
- a history of who saved or reset the mail, and when.

A mail the merchant has not changed, or has reset, goes out in the built-in wording.
The built-in wording is the template the editor starts from.

Templates use a small template language:

- `{{ order.reference }}` prints a variable;
- `{% for line in order.lines %} … {% endfor %}` goes through a list;
- `{% if order.discount %} … {% endif %}` covers a part that may be missing.

Each mail has its own variables, all listed in the editor:

| Mail | Variables |
| --- | --- |
| Sign-in code | `shop.*`, `code`, `valid_minutes` |
| Order confirmation | `shop.*`, `order.reference`, `order.email`, `order.lines` (`name`, `variant`, `quantity`, `amount`), `order.subtotal`, `order.discount` (`code`, `amount`), `order.shipping_method` (`name`, `amount`, `free`), `order.total`, `order.delivery` (`min_days`, `max_days`), `order.shipping_address` and `order.billing_address` (`name`, `line1`, `line2`, `postal_code`, `city`, `country`), `order.note`, `link`, `payment_held` |
| Payment taken | the order confirmation's, without `payment_held` |
| Order cancelled | the order confirmation's, with `refunded` instead of `payment_held` |
| Order shipped | the order confirmation's, with `tracking` and `tracking_url` instead of `payment_held` |
| Review request | `shop.*`, `order.reference`, `products`, `link`, `valid_days` |

The order mails follow the order. The confirmation goes out once the
order is placed: its card is only held, and `payment_held` is true when the money is
taken later, once the order is placed with its supplier (see `docs/payments.md`). The
payment mail goes out once the money is taken, after the invoice is issued, so the
order's page it links to shows the invoice. An order from the shop's own stock is
charged at once and gets both within seconds. The cancellation goes out once the order
is cancelled, by the merchant, the shopper or AliExpress, with its money settled:
`refunded` is true when the money had been taken and goes back, false when the hold on
the card was given back. The shipping mail goes out once the order is fulfilled, by
the merchant or from AliExpress's parcels: `tracking` is the parcel number when one
was given, `tracking_url` the carrier's page only when the merchant gave one at
fulfilment (the shop links to no tracking site of its own choosing; the order's page
the mail links to shows the parcel's steps). Each is sent once
per order by the mailroom (see [Sending in the background](#sending-in-the-background)),
within a day of what prompted it (a node catching up on older events mails nobody),
and the order's page in the back office says when each went out, or why it has not.

Every mail also has `shop.logo_url`, `shop.accent` and `shop.footer`. Set them under
"Look of your mails" on the same page. Each is empty until set, so check it with
`{% if %}`. The built-in wording shows the logo, the accent colour and the footer once
they are set. A shop with none of them set sends exactly what it sent before.

The logo is an uploaded JPEG, PNG, WebP or GIF of at most 8 MiB. It is kept like a
product picture: copied to a majority of nodes before it is recorded. The other nodes
fetch it as they follow the platform's stream. The accent colour is `#rrggbb`. The
footer is plain text of at most 500 characters, and its line breaks are kept.

Amounts are already worded with their currency, for example `12.50 €`.

On save, a template is refused with the reason, and nothing is recorded, when:

- it does not parse, for example a `{% for %}` with no `{% endfor %}` (the line is given);
- it names a variable the mail does not have;
- its HTML holds `<script>`, `<iframe>`, `<object>`, `<embed>`, `<form>`, `<base>`,
  `<link>` or `<meta http-equiv>`, or an `on…` event handler;
- a link or picture address does not start with `https://`, `http://`, `mailto:`,
  `tel:` or `#`, and is not one of the mail's link variables (such as `{{ link }}`).
  Raster `data:image/…` pictures are allowed;
- it fails to render with sample data, both with every optional part filled in and
  with every optional part empty. This catches a `{{ order.discount.code }}` that is
  not inside an `{% if order.discount %}`;
- what it renders to fails the markup check above. The check reads the template as
  written and then every mail rendered from it (the samples on save, the real one on
  every send), with character references decoded as a mail app decodes them: a tag
  spelt out of pieces (`<{% if true %}script{% endif %}>`) or a `javascript&#58;`
  address is refused all the same. A campaign or an automated mail must also carry a
  real link to its unsubscribe page in the rendered mail, not merely mention the
  variable.

A plain-text version goes through the same checks, except the markup one: it must
parse, name only the mail's variables, render with both samples, and stay under
100 KB. Saving the mail without one goes back to the derived text.

Safety rules:

- Every variable is HTML-escaped and the `safe` filter is unavailable, so what a
  shopper typed (a note, an address) shows as text.
- The subject is plain text on one line.
- The plain-text part of each mail is the merchant's plain-text version, or is
  derived from the HTML when there is none. Variables in the plain text are printed
  as they are, since a mail app shows that part as text, never as markup.
- The dashboard shows the preview in a sandboxed frame, so the merchant's HTML never
  runs on the dashboard's origin.

If a saved template still fails at send time, for example by running out of its
rendering budget, the failure is logged and the built-in wording, plain text included,
is sent instead. A plain-text version that fails alone is logged too, and the text
derived from the HTML is sent with the merchant's HTML. The shopper always gets the mail.

## Several languages

A shop may word each mail in several languages. Under "Languages" on `/t/{slug}/mail`
the merchant picks a default language and any others, from a fixed list of ISO 639-1
codes with their English names. A shop that never picked any mails in English. The
built-in wording is English whatever the shop's languages.

"Mails to shoppers" then lists each mail once per language, each with its own editor at
`/t/{slug}/mail/templates/{kind}/{language}` (the address without a language opens the
default language's), its own plain-text version, history and "Back to the built-in
wording". The editors of the same mail link to one another.

The shopper's language comes from the storefront: the primary subtag of the browser's
`Accept-Language` (`fr` from `fr-FR,fr;q=0.9,en;q=0.8`), sent with the sign-in code
request and with the checkout, and kept on the order (`Cart::LanguageGiven`,
`Order::LanguageSet`). The order page in the dashboard shows it. At send time a mail
goes out:

1. in the shopper's language, when the shop mails in it and has worded the mail in it;
2. else in the shop's default language, when the merchant worded the mail in it;
3. else in the built-in English wording.

A shopper whose language is unknown, or not one the shop mails in, gets the default
language's wording. A mail worded only in a language that is not the default goes out
built-in to everyone else: no other language stands in for the default.

Dropping a language from the list keeps its wordings, out of use, until it is picked
again; they are then used as before.

## Operator settings

```toml
[secrets]
# Seals the shops' mail server passwords (and the payment keys, unless
# payments.secret_key is set). At least 32 characters, the same on every node.
key = "…"

[mail]
# Development and tests only: lets a shop's server be on this machine or a
# private network (the e2e suite points shops at MailDev on localhost:1025).
allow_private_hosts = true
```

- Without `[secrets]`, shops cannot set up a server, and any server already recorded
  is ignored: their mail goes through the `[mail]` server.
- Changing `secrets.key` makes the recorded passwords unreadable. Each merchant then
  enters theirs again.

A shop's server must resolve to public addresses only. Loopback, private, link-local,
shared (CGNAT), unspecified, reserved, documentation and benchmarking addresses are
refused, and the IPv6 transition blocks (NAT64, 6to4, Teredo) with them, both on save
and before every send, so a merchant cannot make the dashboard reach internal
services. The check runs on the name's resolution at that moment, and the SMTP client
resolves the name again when it connects. It narrows the attack, but a name that
changes its answer between the two lookups (DNS rebinding) can still get through:
pinning the SMTP connection to the checked address is a follow-up (the HTTP clients
for webhooks and imported pictures already connect through a resolver that checks
the address dialled).
