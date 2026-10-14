.PHONY: dev storefront storefront.migrate lint fmt fmt-check test check machete e2e

start: up dev

cert:
	mkdir -p .docker/traefik/certs
	sudo mkcert -key-file .docker/traefik/certs/timada.key -cert-file .docker/traefik/certs/timada.crt traefik.localhost timada.localhost storefront.localhost store.timada.localhost

cert.install:
		sudo mkcert -install

up:
	DOCKER_GID=$(getent group docker | cut -d: -f3) docker compose up -d --remove-orphans

stop:
	DOCKER_GID=$(getent group docker | cut -d: -f3) docker compose stop

down:
	DOCKER_GID=$(getent group docker | cut -d: -f3) docker compose down -v --rmi local --remove-orphans

# Build, serve, rebuild on change and live-reload. `topcoat dev` passes no
# arguments to the binary, so the config path travels through the environment
# and `serve` is the default subcommand.
dev:
	TIMADA_CONFIG=config/dev.local.toml topcoat dev -p timada-dashboard

reset:
	cargo run -p timada-dashboard -- --config config/dev.local.toml reset

migrate:
	cargo run -p timada-dashboard -- --config config/dev.local.toml migrate

# The example storefront at https://storefront.localhost, listening on every
# interface so the compose Traefik reaches it. STORE_TOKEN and STORE_MEDIA_URL
# (where the product pictures load from) both come from a tenant's API keys
# page; without the media URL the pictures don't show:
#   STORE_TOKEN=... STORE_MEDIA_URL=https://timada.localhost/media/<tenant_id> make storefront
# It talks to one node's store API (127.0.0.1:4321) unless told otherwise; to
# go through Traefik's balancer over the cluster nodes instead:
#   STORE_ADDR=store.timada.localhost:4320 STORE_TLS_CA=/etc/ssl/certs/ca-certificates.crt ...
storefront:
	HOST=0.0.0.0 cargo run -p storefront-example

storefront.migrate:
	cargo run -p storefront-example -- migrate

# Browser tests on a fresh database behind the compose Traefik, with the compose
# dex container as the OIDC provider (`make up` first).
e2e:
	scripts/e2e.sh

# Run Clippy linter for code quality
lint:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

# Check code formatting (fails if not formatted)
fmt-check:
	cargo fmt --all -- --check

# Auto-fix code formatting
fmt:
	cargo fmt --all

# Run ALL tests (sequential - use test-parallel for faster execution)
test:
	cargo test --workspace

# ============================================================================
# Parallel Test Execution (optimized for speed)
# ============================================================================
# Analysis: 111.92s sequential → ~19.9s parallel (5.6x speedup with make -j)
# Total: 476 tests across 46 test suites
# Usage: make test-parallel -j
#        make test-parallel -j10  (limit to 10 parallel jobs)
# ============================================================================

# Check for unused dependencies (optional: install with 'cargo install cargo-machete')
machete:
	@if command -v cargo-machete >/dev/null 2>&1; then \
		cargo machete --with-metadata; \
	else \
		echo "⚠ cargo-machete not installed. Skipping unused dependency check."; \
		echo "  Install with: cargo install cargo-machete"; \
	fi

# Run all checks: format, lint, machete, and test (CI-ready)
check: test lint fmt-check machete
	@echo "✓ All checks passed!"

