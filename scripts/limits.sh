#!/usr/bin/env bash
# What the rate limits cost (#602): a flood of password sign-ins on one node,
# alone and then as one of three, where every check asks the two others what
# they let through (`dashboard/src/rate_limit.rs`). Prints, for each case,
# the answers per second, how long they took and what each node spent; the
# figures in docs/sizing.md are from here.
#
# The cases, each for LIMITS_SECS (20) seconds by LIMITS_SENDERS (64) senders:
#   - one client on one node, turned away after its tenth try: what a refusal
#     costs when the node's own count says no, and nobody is asked;
#   - the same client spread over the three nodes, as a balancer does: no node
#     let it through ten times, so every refusal asks the two others;
#   - a client per request (another address, another account), every one let
#     through to the password check: what a pass costs;
#   - the spread client with node 2 stopped where it stands (it takes
#     connections and answers nothing), also at LIMITS_RATE (200) requests a
#     second arriving whatever the answers' pace, then with node 2 gone (it
#     refuses connections).
#
# The nodes are built in release unless LIMITS_PROFILE says otherwise and keep
# their files under LIMITS_DIR (data/limits, emptied first); their settings are
# the load test's (config/load-node*.toml) with this machine trusted as a proxy,
# so a request names its client's address. Nothing else is needed: no compose
# stack, no shop. Run inside `nix develop`.
set -euo pipefail
cd "$(dirname "$0")/.."

profile="${LIMITS_PROFILE:-release}"
case "$profile" in
  dev) bin=target/debug ;;
  *) bin="target/$profile" ;;
esac
dir="${LIMITS_DIR:-data/limits}"
secs="${LIMITS_SECS:-20}"
senders="${LIMITS_SENDERS:-64}"
rate="${LIMITS_RATE:-200}"

topcoat asset bundle -p timada-dashboard --profile "$profile"
cargo build --profile "$profile" -p timada-dashboard -p timada-loadtest
# What the build wrote goes to the disk now, not during a case.
sync

for port in 3000 3001 3002 4321 4322 4323 7000 7001 7002; do
  if (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
    echo "port $port is in use: stop what listens there first" >&2
    exit 1
  fi
done

rm -rf "$dir"
mkdir -p "$dir"
for i in 0 1 2; do
  sed -e "s#data/load/#$dir/#" \
    -e 's#^trusted_proxies = .*#trusted_proxies = ["127.0.0.0/8"]#' \
    "config/load-node$i.toml" >"$dir/node$i.toml"
done
# Node 0 without the two others: the same settings, no cluster.
sed -e "s#$dir/node0/#$dir/alone/#" -e '/^\[cluster\]$/,$d' "$dir/node0.toml" >"$dir/alone.toml"

declare -A pid
# Starts the node of settings $1 in the background and waits until it serves.
serve() {
  "$bin/timada-dashboard" --config "$dir/$1.toml" migrate >/dev/null
  "$bin/timada-dashboard" --config "$dir/$1.toml" serve >>"$dir/$1.log" 2>&1 &
  pid[$1]=$!
}
ready() {
  if ! curl -sf --retry 60 --retry-delay 1 --retry-all-errors -o /dev/null "http://127.0.0.1:$1/ready"; then
    echo "the node on port $1 is not ready: see $dir/*.log" >&2
    exit 1
  fi
}
stop() {
  for name in "$@"; do
    kill -CONT "${pid[$name]}" 2>/dev/null || true
    kill "${pid[$name]}" 2>/dev/null || true
    wait "${pid[$name]}" 2>/dev/null || true
    unset "pid[$name]"
  done
}
trap 'stop "${!pid[@]}"' EXIT

# One client: the same account from the same address.
one_client=(--field email=flood@example.org --field password=wrong)
# A client per request: its own account, and its own address as the proxy
# in front reports it.
many_clients=(
  --field 'email=flood{n}@example.org' --field password=wrong
  --header 'x-forwarded-for={ip}'
)

# Floods the sign-in of the nodes on the ports $2 with the arguments that
# follow, under the title $1; the nodes up are watched.
flood() {
  echo "== $1"
  urls=()
  for port in $2; do urls+=(--url "http://127.0.0.1:$port/auth/email/login"); done
  shift 2
  watch=()
  for name in "${!pid[@]}"; do watch+=(--watch "$name=${pid[$name]}"); done
  "$bin/timada-loadtest" flood "${urls[@]}" \
    --secs "$secs" --concurrency "$senders" "${watch[@]}" "$@"
}

serve alone
ready 3000
flood "one node, one client" 3000 "${one_client[@]}"
flood "one node, a client per request" 3000 "${many_clients[@]}"
stop alone

serve node0
serve node1
serve node2
ready 3000
ready 3001
ready 3002
flood "three nodes, one client on node 0" 3000 "${one_client[@]}"
flood "three nodes, one client spread over the three" "3000 3001 3002" \
  --field email=spread@example.org --field password=wrong
flood "three nodes, a client per request on node 0" 3000 "${many_clients[@]}"

# Each case its own account: the one before left its passes behind.
kill -STOP "${pid[node2]}"
flood "three nodes, node 2 stopped where it stands, one client spread over the two others" \
  "3000 3001" --field email=stopped@example.org --field password=wrong
kill -CONT "${pid[node2]}"
ready 3002
# The two leave node 2 alone for a while yet: past it, their next check
# asks it on the side and finds it back.
sleep 6
flood "three nodes, node 2 answering again, one client spread over the three" \
  "3000 3001 3002" --field email=back@example.org --field password=wrong
kill -STOP "${pid[node2]}"
flood "three nodes, node 2 stopped where it stands, $rate requests a second over the two others" \
  "3000 3001" --field email=paced@example.org --field password=wrong --rate "$rate"
kill -CONT "${pid[node2]}"
ready 3002

stop node2
flood "three nodes, node 2 gone, one client spread over the two others" \
  "3000 3001" --field email=gone@example.org --field password=wrong
