# ─── Stage 1: Build ──────────────────────────────────────
FROM rust:1.85-bookworm AS builder

# Install protoc for gRPC codegen
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Cargo registry mirror (optional, copy .cargo/config.toml if needed)
# COPY .cargo/config.toml .cargo/config.toml

# Cache dependencies — copy manifests first
COPY Cargo.toml Cargo.lock ./
COPY prism-core/Cargo.toml prism-core/Cargo.toml
COPY prism-router/Cargo.toml prism-router/Cargo.toml
COPY prism-admin/Cargo.toml prism-admin/Cargo.toml

# Create stub lib.rs for dependency caching
RUN mkdir -p prism-core/src prism-router/src prism-admin/src && \
    echo "pub fn stub(){}" > prism-core/src/lib.rs && \
    echo "fn main(){}" > prism-router/src/main.rs && \
    echo "fn main(){}" > prism-admin/src/main.rs && \
    cargo build --release -p prism-router 2>/dev/null || true

# Copy actual source
COPY prism-core/ prism-core/
COPY prism-router/ prism-router/
COPY prism-admin/ prism-admin/

# Touch source files to invalidate stubs
RUN touch prism-core/src/lib.rs prism-router/src/main.rs prism-admin/src/main.rs

# Build the real binary (default features: grpc + proxy)
RUN cargo build --release -p prism-router -p prism-admin

# ─── Stage 2: Runtime ────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/prism-router /usr/local/bin/prism-router
COPY --from=builder /build/target/release/prism-admin /usr/local/bin/prism-admin
COPY examples/prism.example.yaml /etc/prism/prism.yaml

RUN mkdir -p /var/log/prism

# HTTP API + Admin
EXPOSE 8080
# Proxy (Pingora reverse proxy)
EXPOSE 8000
# gRPC
EXPOSE 9090
# Admin (same as HTTP by default, separate if configured)
# EXPOSE 8088

ENTRYPOINT ["prism-router"]
CMD ["--config", "/etc/prism/prism.yaml"]
