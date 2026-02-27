.PHONY: all build build-core build-gateway test test-core test-gateway test-ffi test-e2e test-all clean fmt lint

GO := PATH="/usr/local/go/bin:$(PATH)" go

all: build

# --- Build ---
build: build-core build-gateway

build-core:
	cargo build --release

build-gateway: build-core
	cd prism-gateway && $(GO) build -o bin/prism ./cmd/prism

# --- Test ---
test: test-core test-gateway

test-core:
	cargo test

test-gateway:
	cd prism-gateway && $(GO) test ./...

test-ffi: build-core
ifeq ($(shell uname),Darwin)
	cd prism-gateway && DYLD_LIBRARY_PATH=$(CURDIR)/target/release CGO_ENABLED=1 $(GO) test -tags ffi -v ./internal/router/...
else
	cd prism-gateway && LD_LIBRARY_PATH=$(CURDIR)/target/release CGO_ENABLED=1 $(GO) test -tags ffi -v ./internal/router/...
endif

test-e2e:
	cd prism-gateway && $(GO) test -tags e2e -v ./tests/e2e/...

test-all: test-core test-e2e test-ffi

# --- Bench ---
bench:
	cd prism-core && cargo bench

# --- Format ---
fmt:
	cd prism-core && cargo fmt
	cd prism-gateway && gofmt -w .

# --- Lint ---
lint:
	cargo clippy -- -D warnings
	cd prism-gateway && $(GO) vet ./...

# --- Clean ---
clean:
	cargo clean
	cd prism-gateway && rm -rf bin/

# --- Docker ---
docker-up:
	docker compose up -d

docker-down:
	docker compose down

# --- Dev helpers ---
dev-deps:
	docker compose up -d redis postgres
