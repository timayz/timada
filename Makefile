.PHONY: start cert cert.install up stop down dev reset migrate seed.chatounette seed.chatounette.save seed.chatounette.restore snapshot.chatounette storefront storefront.migrate chatounette chatounette.migrate e2e loadtest lint fmt-check fmt test machete check

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

# Fills the "Chatounette" tenant with the snapshot in seeds/chatounette
# (taken by `make snapshot.chatounette`, pictures included), owned by the
# registered user OWNER names, and prints the STORE_* settings for the
# example storefront. Runs with the server stopped:
#   OWNER=me@example.com make seed.chatounette
seed.chatounette:
	cargo run -p timada-dashboard -- --config config/dev.local.toml seed \
		--snapshot seeds/chatounette --tenant Chatounette --owner "$${OWNER:?set OWNER to the owner's e-mail}" \
		--storefront-url "$${STOREFRONT_URL-https://chatounette.localhost}" \
		--pages examples/chatounette/pages --env-file data/seed.chatounette.env

# Keeps the platform as `make seed.chatounette` left it in DEV_SNAPSHOT (the
# server stopped): the store, the read models, the pictures and the shopper
# keys, with the Chatounette clone's database (best once its catalogue is in)
# and the settings the seed printed. Restore brings it all back in seconds,
# replacing what is there: stop the server and Chatounette first. Then
# `make chatounette` with the settings restore prints.
DEV_SNAPSHOT ?= seeds/dev-platform

seed.chatounette.save:
	cargo run -p timada-dashboard -- --config config/dev.local.toml snapshot save $(DEV_SNAPSHOT) --replace \
		$(if $(wildcard chatounette.db),--also chatounette.db) \
		$(if $(wildcard data/seed.chatounette.env),--also data/seed.chatounette.env)

seed.chatounette.restore:
	cargo run -p timada-dashboard -- --config config/dev.local.toml snapshot restore $(DEV_SNAPSHOT) \
		$(if $(wildcard $(DEV_SNAPSHOT)/also/chatounette.db),--also chatounette.db) \
		$(if $(wildcard $(DEV_SNAPSHOT)/also/seed.chatounette.env),--also data/seed.chatounette.env)
	@cat data/seed.chatounette.env 2>/dev/null || true

# Takes the snapshot from the live shop, pictures included (see
# seeds/README.md); run again, it fetches only the pictures not there yet.
snapshot.chatounette:
	cargo run -p timada-catalog-import --bin woo-snapshot -- --site https://www.chatounette.com \
		--out seeds/chatounette --hide ventes-privees \
		--menu-order accessoires-pour-chat,couchage-chat,goodies-chat,jeux-pour-chat,transport-chat,toilette-chat,vetement-pour-chat \
		--tab-stop-at "À propos"

# The example storefront at https://storefront.localhost, listening on every
# interface so the compose Traefik reaches it. STORE_TOKEN and STORE_MEDIA_URL
# (where the product pictures load from) both come from a tenant's API keys
# page; without the media URL the pictures don't show:
#   STORE_TOKEN=... STORE_MEDIA_URL=https://timada.localhost/media/<tenant_id> make storefront
# It talks to one node's store API (127.0.0.1:4321) unless told otherwise; to
# go through Traefik's balancer over the cluster nodes instead:
#   STORE_ADDR=store.timada.localhost:4320 STORE_TLS_CA=/etc/ssl/certs/ca-certificates.crt ...
# Traefik reaches it from the Docker bridge, so its X-Forwarded-For names the
# shopper the dashboard limits sign-in codes for. STORE_API_URL is the dashboard
# the reviews, sign-ins and contact messages go to; the dev one unless set.
storefront:
	HOST=0.0.0.0 TRUSTED_PROXIES=$${TRUSTED_PROXIES-172.16.0.0/12} STORE_API_URL=$${STORE_API_URL-https://timada.localhost} STORE_COOKIE_KEY=$$(printf 'local-dev-cookie-key-%.0s' 1 2) cargo run -p storefront-example

storefront.migrate:
	cargo run -p storefront-example -- migrate

# The Chatounette clone at https://chatounette.localhost, with the settings
# `make seed.chatounette` printed (STORE_API_URL defaults to the dev dashboard):
#   STORE_TOKEN=... STORE_MEDIA_URL=... make chatounette
chatounette:
	HOST=0.0.0.0 PORT=3457 TRUSTED_PROXIES=$${TRUSTED_PROXIES-172.16.0.0/12} STORE_API_URL=$${STORE_API_URL-https://timada.localhost} STORE_COOKIE_KEY=$$(printf 'local-dev-cookie-key-%.0s' 1 2) cargo run -p chatounette-example

chatounette.migrate:
	cargo run -p chatounette-example -- migrate

# Browser tests on a fresh database behind the compose Traefik, with the compose
# dex container as the OIDC provider (`make up` first).
e2e:
	scripts/e2e.sh

# Random shoppers on the three-node cluster through the Chatounette clone,
# on a platform made afresh under data/load (`make up` first); the arguments
# go to timada-loadtest (`ramp` by default):
#   make loadtest ARGS="soak --rate 5 --secs 1800"
loadtest:
	scripts/loadtest.sh $(ARGS)

# Run Clippy linter for code quality
lint:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

# Check code formatting (fails if not formatted)
fmt-check:
	cargo fmt --all -- --check

# Auto-fix code formatting
fmt:
	cargo fmt --all

# Every test of the workspace. The cluster suite is sensitive to load: a
# `preaccept quorum not reached` on a busy machine is the machine, not the
# code; run it again alone.
test:
	cargo test --workspace

# Unused dependencies (cargo-machete is in the dev shell).
machete:
	cargo machete --with-metadata

# What CI runs (.github/workflows/check.yml): tests, lint, formatting, unused
# dependencies. The browser suite (`make e2e`) needs the compose stack and
# stays local.
check: test lint fmt-check machete
	@echo "✓ All checks passed!"

