# ─── Stage 1: Build Rust library ─────────────────────────
FROM rust:1.82 AS rust-builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY prism-core/ prism-core/
RUN cargo build --release -p prism-core

# ─── Stage 2: Build Go binary ────────────────────────────
FROM golang:1.22 AS go-builder
WORKDIR /build

# Copy the entire Rust target/release directory so the CGO LDFLAGS
# path (${SRCDIR}/../../../target/release) resolves correctly.
# prism-gateway/internal/router/ffi.go uses: -L${SRCDIR}/../../../target/release
# SRCDIR = /build/prism-gateway/internal/router
# So ../../../target/release = /build/target/release
COPY --from=rust-builder /build/target/release/libprism_core.so /build/target/release/libprism_core.so
COPY --from=rust-builder /build/target/release/libprism_core.a  /build/target/release/libprism_core.a

# Also put libs in standard path for the linker at runtime
COPY --from=rust-builder /build/target/release/libprism_core.so /usr/local/lib/
COPY --from=rust-builder /build/target/release/libprism_core.a  /usr/local/lib/
RUN ldconfig

# Copy Go source
COPY prism-gateway/ prism-gateway/

WORKDIR /build/prism-gateway
RUN CGO_ENABLED=1 go build -tags ffi -o /build/bin/prism ./cmd/prism/

# ─── Stage 3: Runtime ────────────────────────────────────
FROM debian:bookworm-slim
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=rust-builder /build/target/release/libprism_core.so /usr/local/lib/
COPY --from=go-builder /build/bin/prism /usr/local/bin/prism
COPY configs/prism.example.yaml /etc/prism/prism.yaml
RUN ldconfig

ENV LD_LIBRARY_PATH=/usr/local/lib

EXPOSE 8080
ENTRYPOINT ["prism"]
CMD ["--config", "/etc/prism/prism.yaml"]
