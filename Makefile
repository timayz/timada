.PHONY: dev storefront storefront.migrate chatounette chatounette.migrate seed.chatounette snapshot.chatounette lint fmt fmt-check test check machete e2e

start: up dev

cert:
	mkdir -p .docker/traefik/certs
	sudo mkcert -key-file .docker/traefik/certs/timada.key -cert-file .docker/traefik/certs/timada.crt traefik.localhost timada.localhost storefront.localhost chatounette.localhost store.timada.localhost

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

# Fills the "Chatounette" tenant with the catalogue in seeds/chatounette.json.gz
# (its pictures are fetched from the shop), owned by the registered user
# OWNER names, and prints the STORE_* settings for the example storefront.
# Runs with the server stopped:
#   OWNER=me@example.com make seed.chatounette
seed.chatounette:
	cargo run -p timada-dashboard -- --config config/dev.local.toml seed \
		--snapshot seeds/chatounette.json.gz --tenant Chatounette --owner "$${OWNER:?set OWNER to the owner's e-mail}" \
		--storefront-url "$${STOREFRONT_URL-https://chatounette.localhost}"

# Takes the snapshot again from the live shop (see seeds/README.md).
snapshot.chatounette:
	cargo run -p timada-catalog-import --bin woo-snapshot -- --site https://www.chatounette.com \
		--out seeds/chatounette.json.gz --hide ventes-privees \
		--menu-order accessoires-pour-chat,couchage-chat,goodies-chat,jeux-pour-chat,transport-chat,toilette-chat,vetement-pour-chat

# The example storefront at https://storefront.localhost, listening on every
# interface so the compose Traefik reaches it. STORE_TOKEN and STORE_MEDIA_URL
# (where the product pictures load from) both come from a tenant's API keys
# page; without the media URL the pictures don't show:
#   STORE_TOKEN=... STORE_MEDIA_URL=https://timada.localhost/media/<tenant_id> make storefront
# It talks to one node's store API (127.0.0.1:4321) unless told otherwise; to
# go through Traefik's balancer over the cluster nodes instead:
#   STORE_ADDR=store.timada.localhost:4320 STORE_TLS_CA=/etc/ssl/certs/ca-certificates.crt ...
# Traefik reaches it from the Docker bridge, so its X-Forwarded-For names the
# shopper the dashboard limits sign-in codes for.
storefront:
	HOST=0.0.0.0 TRUSTED_PROXIES=$${TRUSTED_PROXIES-172.16.0.0/12} cargo run -p storefront-example

storefront.migrate:
	cargo run -p storefront-example -- migrate

# The Chatounette clone at https://chatounette.localhost, with the settings
# `make seed.chatounette` printed:
#   STORE_TOKEN=... STORE_MEDIA_URL=... STORE_API_URL=https://timada.localhost make chatounette
chatounette:
	HOST=0.0.0.0 PORT=3457 TRUSTED_PROXIES=$${TRUSTED_PROXIES-172.16.0.0/12} cargo run -p chatounette-example

chatounette.migrate:
	cargo run -p chatounette-example -- migrate

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

