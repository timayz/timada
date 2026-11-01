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

export TIMADA_CONFIG=config/e2e.toml
topcoat asset bundle -p timada-dashboard
cargo build -q -p timada-dashboard -p chatounette-example   # the shop specs spawn the example
./target/debug/timada-dashboard reset   # the database, the pictures and the keys
./target/debug/timada-dashboard serve &
server=$!
trap 'kill "$server" 2>/dev/null; wait "$server" 2>/dev/null || true' EXIT

base_url="${E2E_BASE_URL:-https://timada.localhost}"
curl -sk --retry 30 --retry-delay 1 --retry-all-errors -o /dev/null "$base_url/auth/login"

# Without arguments the whole suite; with them only what they name (a spec path
# or a title filter). The directory must not be passed alongside: Playwright
# ORs filters, so it would match everything again. One worker: the fake Stripe
# and VIES servers the shop specs start bind fixed ports (12112 and 12113) that
# the server config points at, so two spec files must never run at once.
playwright test --workers=1 "${@:-tests/e2e}"
