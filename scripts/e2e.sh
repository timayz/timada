#!/usr/bin/env bash
# Runs the browser suite against a server on a fresh database. Expects the
# compose stack to be up (`make up`): Traefik in front, dex as the OIDC
# provider and MailDev as the mailbox the tests read sign-in codes from. Run
# inside `nix develop`.
set -euo pipefail
cd "$(dirname "$0")/.."

maildev="${MAILDEV_URL:-http://localhost:1080}"
if ! curl -sf -o /dev/null "$maildev/email"; then
  echo "the development mailbox is not reachable at $maildev: run 'make up' first" >&2
  exit 1
fi

# The server under test binds 3000 and the store API 4321; the specs start the
# storefronts and the fake Stripe and VIES servers on the others. With the dev
# server up, `serve` dies on its first bind and the proxy forwards the suite to
# the dev server instead, which writes the test data into its database (#400).
# Checked before the build so a wasted build is not the first sign.
for port in 3000 3457 3458 3459 3460 3461 4321 12112 12113; do
  if (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
    echo "port $port is in use: stop what listens there (the dev server?) first" >&2
    exit 1
  fi
done

export TIMADA_CONFIG=config/e2e.toml
topcoat asset bundle -p timada-dashboard
cargo build -q -p timada-dashboard -p storefront   # the shop specs spawn it
# What the build wrote is still on its way to the disk. Flushed some 30 s later,
# in the middle of the suite, it stalls the server for seconds: answers come too
# late for the tests, and the proxy's health check gives up on the server.
sync
./target/debug/timada-dashboard reset   # the database, the pictures and the keys
./target/debug/timada-dashboard serve &
server=$!
trap 'kill "$server" 2>/dev/null; wait "$server" 2>/dev/null || true' EXIT

# Waits for the server through the proxy, and gives up as soon as the server
# is gone: `set -e` does not see a background process die, and whatever else
# answers the proxy must not get the suite.
base_url="${E2E_BASE_URL:-https://timada.localhost}"
ready=
for _ in $(seq 60); do
  if ! kill -0 "$server" 2>/dev/null; then
    echo "the test server exited before it answered: see its output above" >&2
    exit 1
  fi
  # `-f`: the proxy answers 503 "no available server" on its own until its
  # health check has seen the server, and that must not count as ready.
  if curl -sfk -o /dev/null "$base_url/auth/login"; then
    ready=1
    break
  fi
  sleep 1
done
if [ -z "$ready" ]; then
  echo "the test server did not answer at $base_url within a minute" >&2
  exit 1
fi

# Without arguments the whole suite; with them only what they name (a spec path
# or a title filter). The directory must not be passed alongside: Playwright
# ORs filters, so it would match everything again. One worker: the fake Stripe
# and VIES servers the shop specs start bind fixed ports (12112 and 12113) that
# the server config points at, so two spec files must never run at once.
playwright test --workers=1 "${@:-tests/e2e}"
