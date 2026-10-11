#!/usr/bin/env bash
# Runs the browser suite against a server on a fresh database. Expects the
# compose stack (Traefik) to be up: `make up`. Run inside `nix develop`.
set -euo pipefail
cd "$(dirname "$0")/.."

export TIMADA_CONFIG=config/e2e.toml
topcoat asset bundle -p timada-dashboard
cargo build -q -p timada-dashboard
./target/debug/timada-dashboard reset
./target/debug/timada-dashboard serve &
server=$!
trap 'kill "$server" 2>/dev/null; wait "$server" 2>/dev/null || true' EXIT

base_url="${E2E_BASE_URL:-https://timada.localhost}"
curl -sk --retry 30 --retry-delay 1 --retry-all-errors -o /dev/null "$base_url/auth/login"

playwright test tests/e2e "$@"
