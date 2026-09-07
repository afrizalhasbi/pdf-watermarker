.PHONY: dev build bundle win-bundle win-build test clean

devel: ## run the app in dev mode (vite + tauri)
	cargo tauri dev

build: ## build release binary (frontend + rust)
	cd ts && npm run build
	cd src-tauri && cargo build --release

bundle: ## full installer bundle
	cargo tauri build

win-bundle: ## windows installer bundle (run on a windows machine)
	cargo tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc

win-build: ## windows release exe only (run on a windows machine)
	cargo tauri build --no-bundle --runner cargo-xwin --target x86_64-pc-windows-msvc
	mv /home/host/Core/Watermarker/src-tauri/target/x86_64-pc-windows-msvc/release/app.exe watermarker.exe
	echo Zipping...
	zip watermarker.exe.zip watermarker.exe

test:
	cd src-tauri && cargo test
	cd ts && npm run build

clean:
	cd src-tauri && cargo clean
	rm -rf ts/dist
