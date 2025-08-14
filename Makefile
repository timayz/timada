
dev:
	cargo watch -x 'run -- --log error,timada=debug,evento=debug serve -c ./timada.toml'

tailwind:
	tailwindcss -i ./tailwind.css -o ./assets/main.css --watch
