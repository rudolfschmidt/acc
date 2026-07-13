.PHONY: build clean test

build:
	cargo build --release

clean:
	cargo clean

test:
	cargo test --release
