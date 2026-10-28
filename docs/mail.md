# Mail

The dashboard sends two kinds of mail.

- **Platform mail**: sign-up, sign-in and password-reset codes, and invitations to
  join a tenant. It always goes through the `[mail]` server, from `mail.from`.
- **Shop mail**: what a shop sends its shoppers, meaning order confirmations, review
  requests and storefront sign-in codes. It goes through the shop's own server when
  the merchant set one up, and through the `[mail]` server otherwise. The sender's
  name is the shop's either way.
- **Marketing**: the shop's newsletter campaigns, and the test mail a merchant sends
  themself while writing one. They go through the shop's own server only, never the
  `[mail]` server: a shop without one cannot send a campaign (the send page and the
  test refuse with `mail_server_required`), and one sent before the server was
  removed waits, with nobody claimed, until a server is set up again.

- **Automated mails** of the shop's automation scenarios (see
  `docs/automations.md`) are marketing too: the shop's own server only, each with
  a link that stops them.

## A shop's own server

Members with `mail:manage` set it up on `/t/{slug}/mail`. The form takes:

- the host and port;
- the security: STARTTLS, implicit TLS or none;
- an optional user name and password;
- the address mails come from, plus an optional display name (the shop's name by
  default).

Saving sends a test mail through the new settings to the address given in the form,
which is prefilled with the member's own. The settings are recorded only if that mail
goes out. Leaving the password blank keeps the saved one, as long as the user name
stays the same. "Use the platform's server" removes the settings.

A shop whose server stops accepting mail does not fall back to the platform's server.
The send fails and is logged, so a broken setup shows instead of being hidden. Order
confirmations and review requests are claimed before they are sent, so one that fails
is not retried.

## A shop's own wording

Members with `mail:manage` can also reword the three shop mails, from "Mails to
shoppers" on `/t/{slug}/mail`. Each mail has an editor per language the shop mails in
(see [Several languages](#several-languages)), with:

- the subject and the whole HTML document;
- an optional plain-text version, for mail apps that show no HTML. Left empty, it is
  derived from the HTML;
- a preview with made-up data, plain text included;
- "Send me a test", which mails the draft to the member through the shop's server;
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
| Order confirmation | `shop.*`, `order.reference`, `order.email`, `order.lines` (`name`, `variant`, `quantity`, `amount`), `order.subtotal`, `order.discount` (`code`, `amount`), `order.shipping_method` (`name`, `amount`, `free`), `order.total`, `order.shipping_address` and `order.billing_address` (`name`, `line1`, `line2`, `postal_code`, `city`, `country`), `order.note`, `link` |
| Review request | `shop.*`, `order.reference`, `products`, `link`, `valid_days` |

Every mail also has `shop.logo_url`, `shop.accent` and `shop.footer`. Set them under
"Look of your mails" on the same page. Each is empty until set, so check it with
`{% if %}`. The built-in wording shows the logo, the accent colour and the footer once
they are set. A shop with none of them set sends exactly what it sent before.

The logo is an uploaded JPEG, PNG, WebP or GIF of at most 8 MiB. It is kept like a
product picture: copied to a majority of nodes before it is recorded. The other nodes
fetch it as they follow the platform's stream. The accent colour is `#rrggbb`. The
footer is plain text of at most 500 characters, and its line breaks are kept.

Amounts are already worded with their currency, for example `12.50 EUR`.

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
  not inside an `{% if order.discount %}`.

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
codes with their English names. A shop that never picked any mails in English, and
every wording saved before it could pick is an English wording. The built-in wording is
English whatever the shop's languages.

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
shared (CGNAT) and unspecified addresses are refused, both on save and before every
send, so a merchant cannot make the dashboard reach internal services. The check runs
on the name's resolution at that moment, and the SMTP client resolves the name again
when it connects. It narrows the attack, but a name that changes its answer between
the two lookups (DNS rebinding) can still get through.
