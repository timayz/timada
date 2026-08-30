#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
exec cargo watch -w crates -w apps -x 'run -p demo-store -- serve'
