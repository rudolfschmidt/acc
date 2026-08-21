.PHONY: build test check clean

build:
	cargo build --release

test:
	cargo test

check:
	cargo clippy --release --all-targets

clean:
	cargo clean
