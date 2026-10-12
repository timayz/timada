.PHONY: dev lint fmt fmt-check test check machete e2e

start: up dev

cert:
	mkdir -p .docker/traefik/certs
	sudo mkcert -key-file .docker/traefik/certs/timada.key -cert-file .docker/traefik/certs/timada.crt traefik.localhost timada.localhost

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

# Browser tests on a fresh database behind the compose Traefik (`make up` first).
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

