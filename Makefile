dev:
	cargo watch -x 'run -- --log error,timada=debug,evento=debug serve -c ./timada.toml'

tailwind:
	tailwindcss -i ./tailwind.css -o ./assets/main.css --watch

up:
	docker compose up -d --remove-orphans

stop:
	docker compose stop

down:
	docker compose down -v --remove-orphans

lint:
	cargo clippy --fix --all-features -- -D warnings

test:
	cargo test

fmt:
	cargo fmt -- --emit files

machete:
	cargo machete

advisory.clean:
	rm -rf ~/.cargo/advisory-db

pants: advisory.clean
	cargo pants

audit: advisory.clean
	cargo audit

outdated:
	cargo outdated
