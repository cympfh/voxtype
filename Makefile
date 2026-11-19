.PHONY: build test clean fmt lint

WINDOWS_TARGET := x86_64-pc-windows-gnu
INSTALL_DIR := ~/apps/voxtype

# WSL の GPU パススルー無しの環境では GUI が正常に動かないため、
# Windows ネイティブ実行ファイルをビルドして Windows 側で実行する
build:
	rustup target add $(WINDOWS_TARGET)
	cargo build --release --target $(WINDOWS_TARGET)
	@echo "生成物: target/$(WINDOWS_TARGET)/release/voxtype.exe"

install: build
	mkdir -p $(INSTALL_DIR)
	cp target/$(WINDOWS_TARGET)/release/voxtype.exe $(INSTALL_DIR)

run: install
	$(INSTALL_DIR)/voxtype.exe

test:
	cargo test

clean:
	cargo clean

fmt:
	cargo fmt

lint:
	cargo clippy --all-targets -- -D warnings
