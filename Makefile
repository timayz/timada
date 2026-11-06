.PHONY: start cert cert.install up stop down dev reset migrate seed snapshot.chatounette aliexpress.places storefront storefront.migrate stripe.listen e2e loadtest lint fmt-check fmt test machete check

# What `make seed` writes and `make storefront` reads: the key and the addresses
# of the seeded shop. Git ignores it.
STOREFRONT_SETTINGS := config/storefront.local.env

# Reads those settings into a recipe's shell. A setting in the environment wins
# over the file.
load_settings = keep=$$(export -p | grep ' STORE_' || true); \
	set -a; [ ! -f $(STOREFRONT_SETTINGS) ] || . "$(abspath $(STOREFRONT_SETTINGS))"; set +a; \
	eval "$$keep"

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

# Also removes what only meant something with that store: the storefront's copy
# of its catalogue, which would keep showing the old products, and its key.
reset:
	cargo run -p timada-dashboard -- --config config/dev.local.toml reset
	rm -f storefront.db* $(STOREFRONT_SETTINGS)

migrate:
	cargo run -p timada-dashboard -- --config config/dev.local.toml migrate

# Fills the "Chatounette" tenant with the snapshot in seeds/chatounette
# (taken by `make snapshot.chatounette`, pictures included), owned by the
# registered user OWNER names, links it to a test company unless it already
# has a company, connects it to the Stripe account of config/stripe.local.toml
# when there is one (see docs/payments.md), and writes the storefront's settings
# to STOREFRONT_SETTINGS. Runs with the server stopped; LIMIT loads only the first
# products of the snapshot (a quick seed for trying the shop), and the back office's
# lists catch up when the server next starts, so `make storefront` can follow at once:
#   OWNER=me@example.com make seed
#   LIMIT=200 OWNER=me@example.com make seed
seed:
	cargo run -p timada-dashboard -- --config config/dev.local.toml seed \
		--snapshot seeds/chatounette --tenant Chatounette --owner "$${OWNER:?set OWNER to the owner's e-mail}" \
		$${LIMIT:+--limit "$$LIMIT"} --no-catch-up --test-company \
		--storefront-url "$${STOREFRONT_URL-https://storefront.localhost}" \
		$(if $(wildcard config/stripe.local.toml),--stripe-file config/stripe.local.toml) \
		--pages storefront/pages --env-file $(STOREFRONT_SETTINGS)

# Reads AliExpress's lists of places again (one call per country, with your
# app's key) into crates/aliexpress/places.bin, which the next build embeds.
aliexpress.places:
	cargo run -p timada-dashboard -- --config config/dev.local.toml aliexpress-places

# Takes the snapshot from the live shop, pictures included (see
# seeds/README.md); run again, it fetches only the pictures not there yet.
snapshot.chatounette:
	cargo run -p timada-catalog-import --bin woo-snapshot -- --site https://www.chatounette.com \
		--out seeds/chatounette --hide ventes-privees \
		--menu-order accessoires-pour-chat,couchage-chat,goodies-chat,jeux-pour-chat,transport-chat,toilette-chat,vetement-pour-chat \
		--tab-stop-at "À propos"

# The shop at https://storefront.localhost, listening on every interface so the
# compose Traefik reaches it, with the settings `make seed` wrote. A setting in the
# environment wins over the file, as for another tenant's key (STORE_TOKEN and
# STORE_MEDIA_URL, where the product pictures load from, are on its API keys page):
#   STORE_TOKEN=... STORE_MEDIA_URL=... make storefront
# It talks to one node's store API (127.0.0.1:4321) unless told otherwise; to
# go through Traefik's balancer over the cluster nodes instead:
#   STORE_ADDR=store.timada.localhost:4320 STORE_TLS_CA=/etc/ssl/certs/ca-certificates.crt ...
# Traefik reaches it from the Docker bridge, so its X-Forwarded-For names the
# shopper the dashboard limits sign-in codes for. STORE_API_URL is the dashboard
# the reviews, sign-ins and contact messages go to; the dev one unless set.
storefront:
	@$(load_settings); \
	: "$${STORE_TOKEN:?run make seed first, or set it}"; \
	HOST=0.0.0.0 PORT=3457 TRUSTED_PROXIES=$${TRUSTED_PROXIES-172.16.0.0/12} STORE_API_URL=$${STORE_API_URL-https://timada.localhost} STORE_COOKIE_KEY=$$(printf 'local-dev-cookie-key-%.0s' 1 2) cargo run -p storefront

storefront.migrate:
	cargo run -p storefront -- migrate

# Forwards Stripe's test payments and refunds to the shop `make seed` filled, until stopped;
# `stripe login` first, once (docs/payments.md). It goes to the dev server rather
# than https://timada.localhost, so the CLI does not trip over the mkcert certificate.
stripe.listen:
	@$(load_settings); \
	stripe listen \
		--events payment_intent.amount_capturable_updated,payment_intent.succeeded,payment_intent.canceled,charge.refunded \
		--forward-to http://localhost:3000/stripe/webhooks/$${STORE_TENANT_ID:?run make seed first, or set it}

# Browser tests on a fresh database behind the compose Traefik, with the compose
# dex container as the OIDC provider (`make up` first).
e2e:
	scripts/e2e.sh

# Random shoppers on the three-node cluster through the storefront,
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

# The gate before a pull request: tests, lint, formatting, unused
# dependencies. The browser suite (`make e2e`) needs the compose stack and is
# run separately.
check: test lint fmt-check machete
	@echo "✓ All checks passed!"

