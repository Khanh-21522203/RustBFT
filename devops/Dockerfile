# Multi-stage build for minimal image size (doc 15 section 2)

# Stage 1: Build
FROM rust:1.75-bookworm AS builder
WORKDIR /build
COPY . .
RUN cargo build --release --bin RustBFT

# Stage 2: Runtime
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates libclang-dev && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/RustBFT /usr/local/bin/rustbft-node

# Default directories
RUN mkdir -p /data /config

EXPOSE 26656 26657 26660

ENTRYPOINT ["rustbft-node"]
CMD ["/config/node.toml"]
