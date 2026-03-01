.PHONY: all build test bench fmt lint clean docker-up docker-down dev-deps

all: build

# --- Build ---
build:
	cargo build --release

build-debug:
	cargo build

# --- Test ---
test:
	cargo test

test-verbose:
	cargo test -- --nocapture

# --- Bench ---
bench:
	cargo bench -p prism-core

# --- Format ---
fmt:
	cargo fmt --all

# --- Lint ---
lint:
	cargo clippy --all-targets -- -D warnings

# --- Clean ---
clean:
	cargo clean

# --- Docker ---
docker-build:
	docker compose build

docker-up:
	docker compose up -d

docker-down:
	docker compose down

# --- Dev helpers ---
dev-deps:
	docker compose up -d redis postgres

# --- Run ---
run:
	cargo run --release -p prism-router -- --config configs/prism.yaml

run-debug:
	RUST_LOG=prism_router=debug,prism_core=debug cargo run -p prism-router -- --config configs/prism.yaml

# --- Feature variants ---
build-minimal:
	cargo build --release -p prism-router --no-default-features

build-no-proxy:
	cargo build --release -p prism-router --no-default-features --features grpc

# --- WASM plugins ---
.PHONY: wasm-plugins
wasm-plugins:
	cd examples/wasm-plugins/openai-compat && cargo build --target wasm32-unknown-unknown --release
	cp examples/wasm-plugins/openai-compat/target/wasm32-unknown-unknown/release/prism_openai_compat.wasm prism-router/tests/wasm_fixtures/

.PHONY: test-wasm
test-wasm: wasm-plugins
	cargo test -p prism-router --features wasm
