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
