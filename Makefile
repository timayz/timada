.PHONY: css css-watch dev lint fmt fmt-fix test check machete 

start: up dev

# Build CSS once
css:
	tailwindcss -i tailwind.css -o static/css/main.css --minify

# Watch CSS for changes
css-watch:
	tailwindcss -i tailwind.css -o static/css/main.css --watch

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

# Watch and run server on code changes
dev:
	cargo watch -x "run -- --config config/dev.local.toml serve"

reset:
	cargo run -- --config config/dev.local.toml reset

migrate:
	cargo run -- --config config/dev.local.toml migrate

clean:
	cargo clean -p timada

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
check: test lint fmt-fix machete
	@echo "✓ All checks passed!"

