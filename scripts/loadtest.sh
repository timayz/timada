#!/usr/bin/env bash
# Load-tests the three-node cluster through the Chatounette storefront, on a
# platform made afresh under data/load: builds everything (release unless
# LOAD_PROFILE says otherwise), migrates the three nodes, starts two of them,
# seeds the shop through the third (so the seed's writes reach a quorum),
# starts it, starts the storefront through Traefik's balancer over the
# nodes' store API, waits for the catalogue to show, then runs
# timada-loadtest with the arguments given (`ramp` or `soak` and their
# flags; `timada-loadtest --help`). Expects the compose stack (`make up`:
# Traefik, MailDev, stripe-mock) and the snapshot in seeds/chatounette
# (`make snapshot.chatounette`). Run inside `nix develop`.
#
#   scripts/loadtest.sh ramp --start 2 --factor 1.5 --step-secs 60
#   scripts/loadtest.sh soak --rate 5 --secs 1800
#
# The nodes' and the storefront's logs, and the report, stay in data/load.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ $# -eq 0 ]; then
  set -- ramp
fi

profile="${LOAD_PROFILE:-release}"
case "$profile" in
  dev) bin=target/debug ;;
  *) bin="target/$profile" ;;
esac
dashboard="${LOAD_DASHBOARD:-https://timada.localhost}"
target="${LOAD_TARGET:-https://chatounette.localhost}"
stripe_mock="${LOAD_STRIPE_MOCK:-http://localhost:12111}"

if ! curl -s -o /dev/null "$stripe_mock/v1/balance"; then
  echo "stripe-mock is not reachable at $stripe_mock: run 'make up' first" >&2
  exit 1
fi
if [ ! -f seeds/chatounette/catalog.json.gz ]; then
  echo "no snapshot in seeds/chatounette: run 'make snapshot.chatounette' first" >&2
  exit 1
fi

topcoat asset bundle -p timada-dashboard --profile "$profile"
cargo build --profile "$profile" -p timada-dashboard -p chatounette-example -p timada-loadtest

# LOAD_REUSE=1 keeps the platform a run made (seeding takes a while) and
# starts its three nodes over it again.
reuse=""
if [ "${LOAD_REUSE:-}" = 1 ] && [ -s data/load/seed.env ]; then
  reuse=1
  rm -f data/load/report.json
else
  rm -rf data/load
  mkdir -p data/load
fi
pids=()
cleanup() {
  for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
  wait 2>/dev/null || true
}
trap cleanup EXIT

# Waits for $1 to answer, for up to $2 seconds.
wait_for() {
  if ! curl -sk --retry "$2" --retry-delay 1 --retry-all-errors -o /dev/null "$1"; then
    echo "$1 did not answer: see data/load/*.log" >&2
    exit 1
  fi
}

node() {
  "$bin/timada-dashboard" --config "config/load-node$1.toml" "${@:2}"
}

# Starts node $1 in the background. The binary itself, not through `node`:
# a function sent to the background is a subshell, and killing it would
# leave the server running.
serve() {
  "$bin/timada-dashboard" --config "config/load-node$1.toml" serve >"data/load/node$1.log" 2>&1 &
  pids+=($!)
}

# A server left from an earlier run would take the ports and answer instead.
for port in 3000 3001 3002 3457 4321 4322 4323 7000 7001 7002 12112; do
  if (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
    echo "port $port is in use: stop what listens there first" >&2
    exit 1
  fi
done

if [ -z "$reuse" ]; then
  for i in 0 1 2; do node "$i" migrate >/dev/null; done
fi
serve 1
serve 2
wait_for http://127.0.0.1:3001/auth/login 60
wait_for http://127.0.0.1:3002/auth/login 60

if [ -z "$reuse" ]; then
  # The seed logs on stdout too: only its STORE_* lines are settings.
  if ! node 0 seed --snapshot seeds/chatounette --tenant Chatounette \
    --owner load@example.org --owner-password load-test-password \
    --storefront-url "$target" --pages examples/chatounette/pages \
    --test-payments >data/load/seed.out 2>&1; then
    echo "the seed failed: see data/load/seed.out" >&2
    exit 1
  fi
  grep '^STORE_' data/load/seed.out >data/load/seed.env
fi
set -a
# shellcheck disable=SC1091
. data/load/seed.env
set +a

serve 0
wait_for http://127.0.0.1:3000/auth/login 60
wait_for "$dashboard/auth/login" 30

[ -f data/load/chatounette.db ] || STORE_DB=data/load/chatounette.db "$bin/chatounette" migrate >/dev/null
# The storefront gives up when the store is not answering yet (nodes just
# started, waiting for one another): it is started again until it stays up.
start_storefront() {
  STORE_DB=data/load/chatounette.db \
    STORE_ADDR=store.timada.localhost:4320 \
    STORE_TLS_CA="${STORE_TLS_CA:-/etc/ssl/certs/ca-certificates.crt}" \
    HOST=0.0.0.0 PORT=3457 TRUSTED_PROXIES=172.16.0.0/12 \
    STORE_COOKIE_KEY=load-only-cookie-key-load-only-cookie-key \
    "$bin/chatounette" >>data/load/chatounette.log 2>&1 &
  storefront=$!
}
for _ in $(seq 30); do
  start_storefront
  sleep 3
  if kill -0 "$storefront" 2>/dev/null; then break; fi
done
if ! kill -0 "$storefront" 2>/dev/null; then
  echo "the storefront does not start: see data/load/chatounette.log" >&2
  exit 1
fi
pids+=("$storefront")

# The storefront's copy of the catalogue fills from the store, a while for
# a whole snapshot: the run starts once it stops growing.
products() {
  sqlite3 data/load/chatounette.db "SELECT count(*) FROM storefront_products" 2>/dev/null || echo 0
}
last=-1
still=0
for _ in $(seq 720); do
  count=$(products)
  if [ "$count" -gt 0 ] && [ "$count" = "$last" ]; then
    still=$((still + 1))
    if [ "$still" -ge 4 ]; then break; fi
  else
    still=0
    echo "storefront: $count products" >&2
  fi
  last=$count
  sleep 5
done
if [ "$still" -lt 4 ]; then
  echo "the storefront's catalogue is still filling after an hour: see data/load/chatounette.log" >&2
  exit 1
fi

# pids[0..1] are nodes 1 and 2, pids[2] node 0.
watch=(
  --watch "node1=${pids[0]}" --watch "node2=${pids[1]}" --watch "node0=${pids[2]}"
  --watch "storefront=$storefront"
)
for i in 0 1 2; do
  watch+=(--watch-file "data/load/node$i/store.db" --watch-file "data/load/node$i/store.db-wal")
done

"$bin/timada-loadtest" "$@" --target "$target" --dashboard "$dashboard" \
  --stripe-mock "$stripe_mock" --report data/load/report.json "${watch[@]}"
