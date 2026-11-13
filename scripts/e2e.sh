#!/usr/bin/env bash
# Runs the browser suite against a server on a fresh database. Expects the
# compose stack to be up (`make up`): Traefik in front, dex as the OIDC
# provider and MailDev as the mailbox the tests read sign-in codes from. Run
# inside `nix develop`.
#
# E2E_NODES=3 (`make e2e.cluster`) runs it against a cluster of three nodes
# instead. The proxy sends each request to the next node, so a form's post and
# the page after it are answered by different nodes: the one setup where a
# read from a node's own file and a read through the cluster differ. It takes
# longer, and each node's output goes to data/e2e/node<N>.log.
set -euo pipefail
cd "$(dirname "$0")/.."

nodes="${E2E_NODES:-1}"
if [ "$nodes" != 1 ] && [ "$nodes" != 3 ]; then
  echo "E2E_NODES is 1 or 3 (the proxy balances over three ports), not '$nodes'" >&2
  exit 1
fi
# The specs read it: a node's page says how many nodes there are.
export E2E_NODES="$nodes"

maildev="${MAILDEV_URL:-http://localhost:1080}"
if ! curl -sf -o /dev/null "$maildev/email"; then
  echo "the development mailbox is not reachable at $maildev: run 'make up' first" >&2
  exit 1
fi

# The server under test binds 3000 and the store API 4321; the specs start the
# storefronts and the fake Stripe and VIES servers on the others. With the dev
# server up, `serve` dies on its first bind and the proxy forwards the suite to
# the dev server instead, which writes the test data into its database (#400).
# The proxy balances over 3000 to 3002, so a server left on 3001 or 3002 would
# get its share of the suite too. Checked before the build so a wasted build is
# not the first sign.
ports=(3000 3001 3002 3457 3458 3459 3460 3461 4321 12112 12113)
if [ "$nodes" = 3 ]; then
  # The other nodes' store API, and what the nodes talk to one another on.
  ports+=(4322 4323 7000 7001 7002)
fi
for port in "${ports[@]}"; do
  if (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
    echo "port $port is in use: stop what listens there (the dev server?) first" >&2
    exit 1
  fi
done

topcoat asset bundle -p timada-dashboard
cargo build -q -p timada-dashboard -p storefront   # the shop specs spawn it
# What the build wrote is still on its way to the disk. Flushed some 30 s later,
# in the middle of the suite, it stalls the server for seconds: answers come too
# late for the tests, and the proxy's health check gives up on the server.
sync

# Each server's process, how a message names it and where its output goes.
servers=()
names=()
logs=()
stop_servers() {
  local pid
  for pid in "${servers[@]}"; do kill "$pid" 2>/dev/null || true; done
  for pid in "${servers[@]}"; do wait "$pid" 2>/dev/null || true; done
}
trap stop_servers EXIT

if [ "$nodes" = 1 ]; then
  export TIMADA_CONFIG=config/e2e.toml
  ./target/debug/timada-dashboard reset   # the database, the pictures and the keys
  ./target/debug/timada-dashboard serve &
  servers+=($!)
  names+=("the test server")
  logs+=("its output above")
else
  # Every node runs on config/e2e.toml with the cluster they form added, and
  # takes what differs between them, its ports and its files, from the
  # environment, which wins over the file.
  mkdir -p data/e2e
  {
    cat config/e2e.toml
    cat <<'EOF'

# Added by scripts/e2e.sh: the three nodes of the run, all on this machine.
[cluster]
peers = ["127.0.0.1:7000", "127.0.0.1:7001", "127.0.0.1:7002"]
web_peers = ["http://127.0.0.1:3000", "http://127.0.0.1:3001", "http://127.0.0.1:3002"]
secret = "e2e-only-cluster-secret-never-used-outside-the-test-suite"
EOF
  } >data/e2e/cluster.toml
  for i in 0 1 2; do
    node=(
      env TIMADA_CONFIG=data/e2e/cluster.toml
      "TIMADA__CLUSTER__NODE_ID=$i"
      "TIMADA__SERVER__PORT=$((3000 + i))"
      "TIMADA__STORE_API__LISTEN=127.0.0.1:$((4321 + i))"
      "TIMADA__DATABASE__PATH=data/e2e/node$i/timada.db"
      "TIMADA__MEDIA__DIR=data/e2e/node$i/media"
      "TIMADA__DATA_KEYS__DIR=data/e2e/node$i/shopper-keys"
      ./target/debug/timada-dashboard
    )
    "${node[@]}" reset
    "${node[@]}" serve >"data/e2e/node$i.log" 2>&1 &
    servers+=($!)
    names+=("node $i")
    logs+=("data/e2e/node$i.log")
  done
fi

# Stops the run when a server is gone: `set -e` does not see a background
# process die, and whatever else answers the proxy must not get the suite.
check_servers() {
  local i
  for i in "${!servers[@]}"; do
    if ! kill -0 "${servers[$i]}" 2>/dev/null; then
      echo "${names[$i]} exited: see ${logs[$i]}" >&2
      exit 1
    fi
  done
}

# Runs "$@" every second until it succeeds, for up to a minute.
wait_for() {
  for _ in $(seq 60); do
    check_servers
    if "$@"; then
      return 0
    fi
    sleep 1
  done
  return 1
}

# `-f`: the proxy answers 503 "no available server" on its own until its
# health check has seen the server, and that must not count as an answer.
answers() {
  curl -sfk -o /dev/null "$1"
}

# Waits for every node of the cluster to say it is ready, as it tells the
# proxy: it takes writes and its read models keep up with the store.
nodes_ready() {
  local i
  for i in "${!servers[@]}"; do
    if ! wait_for answers "http://127.0.0.1:$((3000 + i))/ready"; then
      echo "${names[$i]} is not ready: see ${logs[$i]}" >&2
      exit 1
    fi
  done
}

# Whether the proxy sends to every node: its health check passed on each.
balanced() {
  [ "$(curl -sk https://traefik.localhost/api/http/services/timada-dev@file | grep -o '"UP"' | wc -l)" -eq "$nodes" ]
}

if [ "$nodes" = 3 ]; then
  nodes_ready
  if ! wait_for balanced; then
    echo "the nodes are ready, but the proxy does not send to all three: see 'docker compose logs traefik'" >&2
    exit 1
  fi
fi
# Through the proxy, as the suite goes.
base_url="${E2E_BASE_URL:-https://timada.localhost}"
if ! wait_for answers "$base_url/auth/login"; then
  echo "the test server did not answer at $base_url within a minute" >&2
  exit 1
fi

# Without arguments the whole suite; with them only what they name (a spec path
# or a title filter). The directory must not be passed alongside: Playwright
# ORs filters, so it would match everything again. One worker: the fake Stripe
# and VIES servers the shop specs start bind fixed ports (12112 and 12113) that
# the server config points at, so two spec files must never run at once.
status=0
playwright test --workers=1 "${@:-tests/e2e}" || status=$?

# The proxy leaves out a node that died or is no longer ready, and the suite
# goes on with the others: that must not pass for a green run on three nodes.
check_servers
if [ "$nodes" = 3 ]; then
  nodes_ready
  echo "the nodes' output: data/e2e/node0.log, node1.log and node2.log" >&2
fi
exit "$status"
