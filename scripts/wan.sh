#!/usr/bin/env bash
# Nine nodes in three regions, on one machine (#725): three releases of the
# chart in three namespaces of one kind cluster stand in for three Kubernetes
# clusters, and a delay is put between the pods of different regions, 10 ms
# there and back inside Europe (r0, r1) and 80 ms across the ocean (r2).
# Measured through one pod of each region: an account registered, a sign-in
# refused, `/ready`; with no delay, with it, with the far region cut off and
# healed, and with 1 % of the packets lost. docs/sizing.md has the figures.
#
# Needs docker, and kind, helm and kubectl on the path:
#   nix shell nixpkgs#kind nixpkgs#kubernetes-helm nixpkgs#kubectl --command make wan
# WAN_IMAGE names an image already built (timayz/timada:<tag>); WAN_KEEP=1
# leaves the cluster up; WAN_COUNT is how many of each request (20).
set -euo pipefail
cd "$(dirname "$0")/.."

NAME=timada-wan
DIR=data/wan
COUNT=${WAN_COUNT:-20}
EU_MS=${WAN_EU_MS:-5}   # one way: 10 ms there and back
US_MS=${WAN_US_MS:-40}  # one way: 80 ms there and back
mkdir -p "$DIR"
export KUBECONFIG="$PWD/$DIR/kubeconfig"

if [ -z "${WAN_IMAGE:-}" ]; then
  WAN_IMAGE=wan
  docker build -t timayz/timada:wan . >"$DIR/build.log" 2>&1
fi

kind delete cluster --name "$NAME" >/dev/null 2>&1 || true
kind create cluster --name "$NAME" >/dev/null 2>&1
[ -n "${WAN_KEEP:-}" ] || trap 'kind delete cluster --name "$NAME" >/dev/null 2>&1' EXIT
kind load docker-image "timayz/timada:$WAN_IMAGE" --name "$NAME" >/dev/null 2>&1
kubectl wait --for=condition=Ready nodes --all --timeout=120s >/dev/null

# The whole cluster's list, the same in every release: each pod by its
# namespace's name for it.
peers=()
for region in 0 1 2; do
  for pod in 0 1 2; do
    id=$((region * 3 + pod))
    peers+=("timada-$id.timada-headless.r$region.svc.cluster.local:7000")
  done
done
join() { local IFS=,; echo "$*"; }
cat >"$DIR/values.yaml" <<VALUES
image: {tag: $WAN_IMAGE}
replicas: 3
persistence: {storageClassName: standard, longhornCritical: false, size: 1Gi}
extraSecretEnv:
  TIMADA__AUTH__SECRET_KEY: "0123456789abcdef0123456789abcdef-auth"
  TIMADA__SECRETS__KEY: "0123456789abcdef0123456789abcdef-secr"
  TIMADA__MAIL__SMTP_URL: "smtp://127.0.0.1:1025"
  TIMADA__CLUSTER__SECRET: "0123456789abcdef0123456789abcdef-clus"
peers: [$(join "${peers[@]}")]
VALUES
for region in 0 1 2; do
  kubectl create namespace "r$region" >/dev/null
  # Without the gateway's route and certificate: nothing is served outside,
  # and the certificate has one name for every release.
  helm template timada helm -n "r$region" -f "$DIR/values.yaml" --set "firstNodeId=$((region * 3))" |
    awk 'BEGIN{RS="\n---\n"; ORS="\n---\n"} !/kind: (Certificate|HTTPRoute)/' |
    kubectl apply -n "r$region" -f - >/dev/null
done
for _ in $(seq 90); do
  [ "$(kubectl get pods -A -l app=timada --no-headers 2>/dev/null | grep -c '1/1')" = 9 ] && break
  sleep 4
done
kubectl get pods -A -l app=timada --no-headers \
  -o custom-columns=N:.metadata.name,IP:.status.podIP,R:.metadata.namespace >"$DIR/pods.txt"
[ "$(wc -l <"$DIR/pods.txt")" = 9 ] || { echo "the nine nodes did not all start"; exit 1; }

kubectl create configmap wan --from-file=scripts/wan/delay.sh --from-file=scripts/wan/measure.sh \
  --from-file="$DIR/pods.txt" >/dev/null
mount='"volumeMounts":[{"name":"s","mountPath":"/s"}]'
volume='"volumes":[{"name":"s","configMap":{"name":"wan"}}]'
# `net` sees the machine's processes and may change a pod's network; `probe`
# asks the nodes, with no delay of its own.
kubectl run net --image=nicolaka/netshoot --restart=Never --overrides="{\"spec\":{\"hostPID\":true,\"containers\":[{\"name\":\"net\",\"image\":\"nicolaka/netshoot\",\"command\":[\"sleep\",\"36000\"],\"securityContext\":{\"privileged\":true},$mount}],$volume}}" >/dev/null
kubectl run probe --image=curlimages/curl --restart=Never --overrides="{\"spec\":{\"containers\":[{\"name\":\"probe\",\"image\":\"curlimages/curl\",\"command\":[\"sleep\",\"36000\"],$mount}],$volume}}" >/dev/null
kubectl wait --for=condition=Ready pod/net pod/probe --timeout=300s >/dev/null

delay() { kubectl exec net -- sh /s/delay.sh "$@" >/dev/null; }
measure() { kubectl exec probe -- sh /s/measure.sh "$@"; }
ip_of() { awk -v pod="$1" '$1 == pod {print $2}' "$DIR/pods.txt"; }
ready_code() { kubectl exec probe -- curl -s -m 3 -o /dev/null -w '%{http_code}' "http://$(ip_of "$1"):3000/ready" || true; }
# Seconds until `/ready` of a pod answers with a status, a tenth at a time.
until_ready_is() {
  local pod=$1 want=$2 started
  started=$(date +%s%N)
  for _ in $(seq 600); do
    [ "$(ready_code "$pod")" = "$want" ] && break
    sleep 0.1
  done
  echo "$((($(date +%s%N) - started) / 1000000)) ms"
}

echo "== no delay"
measure none "$COUNT"

echo "== $((EU_MS * 2)) ms there and back inside Europe, $((US_MS * 2)) ms across the ocean"
delay on "$EU_MS" "$US_MS"
sleep 15
measure wan "$COUNT"

echo "== the far region cut off"
delay cut r2
echo "timada-6, cut off, answers /ready with 503 after $(until_ready_is timada-6 503)"
measure cut "$COUNT" "timada-0 timada-3"
kubectl exec probe -- curl -s -m 20 -o /dev/null \
  -w 'an account registered through timada-6, cut off: %{http_code} in %{time_total} s\n' \
  -d email=cut@example.com -d password=Correct-horse-battery-9 "http://$(ip_of timada-6):3000/auth/register" || true
# Past the minute a work group goes without a node before another takes it.
sleep 75
echo "work groups of the far region taken on meanwhile: $(for pod in 0 1 2 3 4 5; do kubectl logs -n "r$((pod / 3))" "timada-$pod" -c timada | grep -c 'this node runs its subscriptions' || true; done | awk '{s += $1} END {print s + 0}') of 3"

echo "== healed"
delay on "$EU_MS" "$US_MS"
echo "timada-6 answers /ready with 200 after $(until_ready_is timada-6 200)"
kubectl exec probe -- curl -s -m 20 -o /dev/null \
  -w 'an account registered through it: %{http_code} in %{time_total} s\n' \
  -d email=healed@example.com -d password=Correct-horse-battery-9 "http://$(ip_of timada-6):3000/auth/register" || true
sleep 30
echo "work groups handed back: $(for pod in 0 1 2 3 4 5; do kubectl logs -n "r$((pod / 3))" "timada-$pod" -c timada | grep -c 'its subscriptions are handed back' || true; done | awk '{s += $1} END {print s + 0}') of 3"

echo "== the same delays, 1 % of the packets lost between regions"
delay on "$EU_MS" "$US_MS" 1
sleep 10
measure lossy "$COUNT"

echo "== what the nodes warned of, the mails aside (there is no mail server here)"
for pod in 0 3 6; do
  echo "timada-$pod: $(kubectl logs -n "r$((pod / 3))" "timada-$pod" -c timada | grep '"WARN"\|"ERROR"' | grep -v 'mail\|code not sent' | grep -o '"message":"[^"]*"' | sort | uniq -c | sort -rn | head -5 | sed 's/"message"://' | paste -sd';')"
done

echo "== the requests a node failed, and why"
for pod in 0 3 6; do
  kubectl logs -n "r$((pod / 3))" "timada-$pod" -c timada | grep 'request failed with an internal server error' |
    grep -o '"timestamp":"[^"]*"\|"error":"[^"]\{0,220\}' | paste -sd' ' | sed "s/^/timada-$pod: /" || true
done
