# Mail

The dashboard sends two kinds of mail.

- **Platform mail**: sign-up, sign-in and password-reset codes, and invitations to
  join a tenant. It always goes through the `[mail]` server, from `mail.from`.
- **Shop mail**: what a shop sends its shoppers, meaning order confirmations, review
  requests and storefront sign-in codes. It goes through the shop's own server when
  the merchant set one up, and through the `[mail]` server otherwise. The sender's
  name is the shop's either way.

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
shoppers" on `/t/{slug}/mail`. Each mail has an editor with:

- the subject and the whole HTML document;
- a preview with made-up data;
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
| Sign-in code | `shop.name`, `code`, `valid_minutes` |
| Order confirmation | `shop.name`, `order.reference`, `order.email`, `order.lines` (`name`, `variant`, `quantity`, `amount`), `order.subtotal`, `order.discount` (`code`, `amount`), `order.shipping_method` (`name`, `amount`, `free`), `order.total`, `order.shipping_address` and `order.billing_address` (`name`, `line1`, `line2`, `postal_code`, `city`, `country`), `order.note`, `link` |
| Review request | `shop.name`, `order.reference`, `products`, `link`, `valid_days` |

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

Safety rules:

- Every variable is HTML-escaped and the `safe` filter is unavailable, so what a
  shopper typed (a note, an address) shows as text.
- The subject is plain text on one line.
- The plain-text part of each mail is derived from its HTML.
- The dashboard shows the preview in a sandboxed frame, so the merchant's HTML never
  runs on the dashboard's origin.

If a saved template still fails at send time, for example by running out of its
rendering budget, the failure is logged and the built-in wording is sent instead. The
shopper always gets the mail.

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
