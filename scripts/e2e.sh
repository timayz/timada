#!/usr/bin/env bash
# Runs the browser suite against servers on fresh databases. Expects the
# compose stack to be up (`make up`): Traefik in front, dex as the OIDC
# provider and MailDev as the mailbox the tests read sign-in codes from. Run
# inside `nix develop`.
#
# The spec files run side by side on E2E_SHARDS servers (three by default),
# each on its own database and ports, with a Playwright worker each: worker N
# drives server N at https://s<N>.timada.localhost (tests/e2e/shard.ts), and
# the server's output goes to data/e2e/shard<N>.log. With E2E_SHARDS=1 one
# server at https://timada.localhost runs them one after the other, its output
# in the terminal.
#
# E2E_NODES=3 (`make e2e.cluster`) runs them one after the other against a
# cluster of three nodes instead. The proxy sends each request to the next
# node, so a form's post and the page after it are answered by different
# nodes: the one setup where a read from a node's own file and a read through
# the cluster differ. It takes longer, and each node's output goes to
# data/e2e/node<N>.log.
set -euo pipefail
cd "$(dirname "$0")/.."

nodes="${E2E_NODES:-1}"
if [ "$nodes" != 1 ] && [ "$nodes" != 3 ]; then
  echo "E2E_NODES is 1 or 3 (the proxy balances over three ports), not '$nodes'" >&2
  exit 1
fi
# The specs read it: a node's page says how many nodes there are.
export E2E_NODES="$nodes"

# The proxy reaches s0 to s2.timada.localhost on ports 3000 to 3002, which a
# cluster takes for its nodes.
if [ "$nodes" = 3 ]; then
  shards="${E2E_SHARDS:-1}"
else
  shards="${E2E_SHARDS:-3}"
fi
case "$nodes:$shards" in
  1:1 | 1:2 | 1:3 | 3:1) ;;
  *)
    echo "E2E_SHARDS is 1 to 3, and 1 with E2E_NODES=3, not '$shards'" >&2
    exit 1
    ;;
esac
# The specs read it, and Playwright runs as many workers (tests/e2e/shard.ts).
export E2E_SHARDS="$shards"

maildev="${MAILDEV_URL:-http://localhost:1080}"
if ! curl -sf -o /dev/null "$maildev/email"; then
  echo "the development mailbox is not reachable at $maildev: run 'make up' first" >&2
  exit 1
fi

# The server under test binds 3000 and the store API 4321; the specs start the
# storefronts and the fake Stripe, VIES and AliExpress servers on the others.
# With the dev server up, `serve` dies on its first bind and the proxy forwards
# the suite to the dev server instead, which writes the test data into its
# database (#400). The proxy balances over 3000 to 3002, so a server left on
# 3001 or 3002 would get its share of the suite too. Checked before the build
# so a wasted build is not the first sign.
ports=(3000 3001 3002 3457 3458 3459 3460 3461 4321 12112 12113 12114)
if [ "$nodes" = 3 ]; then
  # The other nodes' store API, and what the nodes talk to one another on.
  ports+=(4322 4323 7000 7001 7002)
fi
# The other servers' store API, and the fakes their workers start, 100 ports
# further on for each next server.
for ((i = 1; i < shards; i++)); do
  ports+=($((4321 + i)) $((12112 + 100 * i)) $((12113 + 100 * i)) $((12114 + 100 * i)))
done
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

if [ "$nodes" = 3 ]; then
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
elif [ "$shards" = 1 ]; then
  export TIMADA_CONFIG=config/e2e.toml
  ./target/debug/timada-dashboard reset   # the database, the pictures and the keys
  ./target/debug/timada-dashboard serve &
  servers+=($!)
  names+=("the test server")
  logs+=("its output above")
else
  # Every server runs on config/e2e.toml and takes what differs between them
  # from the environment, which wins over the file: its ports and its files,
  # the address its worker reaches it at, and the fakes its worker starts.
  mkdir -p data/e2e
  for ((i = 0; i < shards; i++)); do
    url="https://s$i.timada.localhost"
    off=$((100 * i))
    server=(
      env TIMADA_CONFIG=config/e2e.toml
      "TIMADA__SERVER__PORT=$((3000 + i))"
      "TIMADA__SERVER__URL=$url"
      "TIMADA__STORE_API__LISTEN=127.0.0.1:$((4321 + i))"
      "TIMADA__STORE_API__PUBLIC_ADDR=localhost:$((4321 + i))"
      "TIMADA__DATABASE__PATH=data/e2e/shard$i/timada.db"
      "TIMADA__MEDIA__DIR=data/e2e/shard$i/media"
      "TIMADA__MEDIA__PUBLIC_URL=$url/media"
      "TIMADA__DATA_KEYS__DIR=data/e2e/shard$i/shopper-keys"
      "TIMADA__PAYMENTS__STRIPE_API_URL=http://127.0.0.1:$((12112 + off))"
      "TIMADA__INVOICING__VIES_URL=http://127.0.0.1:$((12113 + off))"
      "TIMADA__INVOICING__EURO_RATES_URL=http://127.0.0.1:$((12114 + off))"
      "TIMADA__ALIEXPRESS__API_URL=http://127.0.0.1:$((12114 + off))"
      "TIMADA__ALIEXPRESS__AUTHORIZE_URL=http://127.0.0.1:$((12114 + off))/oauth/authorize"
      ./target/debug/timada-dashboard
    )
    "${server[@]}" reset
    "${server[@]}" serve >"data/e2e/shard$i.log" 2>&1 &
    servers+=($!)
    names+=("server $i")
    logs+=("data/e2e/shard$i.log")
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

# Waits for every server to say it is ready, as it tells the proxy: it takes
# writes and its read models keep up with the store.
servers_ready() {
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
  servers_ready
  if ! wait_for balanced; then
    echo "the nodes are ready, but the proxy does not send to all three: see 'docker compose logs traefik'" >&2
    exit 1
  fi
fi
if [ "$shards" = 1 ]; then
  # Through the proxy, as the suite goes.
  base_url="${E2E_BASE_URL:-https://timada.localhost}"
  if ! wait_for answers "$base_url/auth/login"; then
    echo "the test server did not answer at $base_url within a minute" >&2
    exit 1
  fi
else
  servers_ready
  for ((i = 0; i < shards; i++)); do
    if ! wait_for answers "https://s$i.timada.localhost/auth/login"; then
      echo "the proxy does not reach server $i at https://s$i.timada.localhost: 'docker compose restart traefik' reads .docker/traefik/dynamic.yaml again" >&2
      exit 1
    fi
  done
  # The storefronts the specs start check the certificate of the server they
  # call, which `make cert` makes for every *.timada.localhost.
  if ! curl -sf -o /dev/null "https://s0.timada.localhost/auth/login"; then
    echo "the proxy's certificate does not name s0.timada.localhost: run 'make cert', then 'docker compose restart traefik'" >&2
    exit 1
  fi
  # dex sends a sign-in back only to the addresses .docker/dex/config.yaml
  # listed when it started, and answers any other with a 400.
  for ((i = 0; i < shards; i++)); do
    callback="https://s$i.timada.localhost/auth/oauth2/dex/callback"
    answer=$(curl -sL -o /dev/null -w '%{http_code}' "http://localhost:5556/dex/auth?client_id=timada&response_type=code&scope=openid&redirect_uri=$callback" || true)
    if [ "$answer" = 400 ]; then
      echo "dex does not send sign-ins back to $callback: 'docker compose restart dex' reads .docker/dex/config.yaml again" >&2
      exit 1
    fi
  done
fi

# Without arguments the whole suite; with them only what they name (a spec path
# or a title filter). The directory must not be passed alongside: Playwright
# ORs filters, so it would match everything again. One worker per server
# (playwright.config.ts): the fake Stripe, VIES and AliExpress servers a spec
# starts bind the ports its server's config names, so two spec files must
# never share a server.
status=0
playwright test "${@:-tests/e2e}" || status=$?

# The proxy leaves out a node that died or is no longer ready, and the suite
# goes on with the others: that must not pass for a green run on three nodes.
check_servers
if [ "$nodes" = 3 ]; then
  servers_ready
  echo "the nodes' output: data/e2e/node0.log, node1.log and node2.log" >&2
elif [ "$shards" != 1 ]; then
  echo "the servers' output: data/e2e/shard0.log to shard$((shards - 1)).log" >&2
fi
exit "$status"
