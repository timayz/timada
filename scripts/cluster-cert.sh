#!/usr/bin/env bash
# The certificates of a cluster run on this machine: an authority made for it
# and one certificate for 127.0.0.1, which every local node presents (they
# are all at that address, so they could speak as one another: fine here).
# Kept in DIR (data/cluster-tls by default) and made only when missing:
#   scripts/cluster-cert.sh [DIR]
set -euo pipefail
dir=${1:-data/cluster-tls}
[ -s "$dir/tls.crt" ] && [ -s "$dir/tls.key" ] && [ -s "$dir/ca.crt" ] && exit 0
mkdir -p "$dir"
cd "$dir"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -keyout ca.key -out ca.crt -days 3650 -subj "/CN=timada local cluster" \
  -addext "basicConstraints=critical,CA:TRUE" \
  -addext "keyUsage=critical,keyCertSign,cRLSign" 2>/dev/null
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -keyout tls.key -out tls.csr -subj "/CN=127.0.0.1" 2>/dev/null
openssl x509 -req -in tls.csr -CA ca.crt -CAkey ca.key -CAcreateserial \
  -out tls.crt -days 3650 -extfile <(printf '%s\n' \
    "subjectAltName=IP:127.0.0.1" \
    "basicConstraints=critical,CA:FALSE" \
    "keyUsage=critical,digitalSignature" \
    "extendedKeyUsage=serverAuth,clientAuth") 2>/dev/null
rm -f tls.csr ca.srl
