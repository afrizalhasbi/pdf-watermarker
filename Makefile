.PHONY: dev build bundle test clean

dev: ## run the app in dev mode (vite + tauri)
	cargo tauri dev

build: ## build release binary (frontend + rust)
	cd ts && npm run build
	cd src-tauri && cargo build --release

bundle: ## full installer bundle
	cargo tauri build

test:
	cd src-tauri && cargo test
	cd ts && npm run build

clean:
	cd src-tauri && cargo clean
	rm -rf ts/dist
