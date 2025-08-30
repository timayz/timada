dev:
	cargo run -- --log error,timada=debug,evento=debug migrate -c ./timada.toml
	cargo watch -x 'run -- --log error,timada=debug,evento=debug serve -c ./timada.toml'

tailwind:
	tailwindcss -i ./tailwind.css -o ./assets/main.css --watch

reset:
	cargo run -- --log error,timada=debug,evento=debug reset -c ./timada.toml

cert:
	mkdir -p .docker/traefik/certs
	mkcert -key-file .docker/traefik/certs/timada.key -cert-file .docker/traefik/certs/timada.crt timada.localhost traefik.localhost *.timada.localhost

cert.install:
	mkcert -install

up:
	sudo docker compose up -d --remove-orphans

stop:
	sudo docker compose stop

down:
	sudo docker compose down -v --rmi local --remove-orphans

lint:
	cargo clippy --fix --all-features -- -D warnings

test:
	cargo test

e2e:
	npx playwright test --headed

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
