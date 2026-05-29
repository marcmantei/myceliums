# ── Stage 1: Build ────────────────────────────────────────────────────────────
FROM rust:1.88-bookworm AS builder

RUN apt-get update && apt-get install -y \
    protobuf-compiler \
    cmake \
    pkg-config \
    libzstd-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY myc/ myc/
COPY crates/ crates/

# Limit parallelism to prevent OOM (signal 11: SIGSEGV) during build
# The mail-parser dependency increases memory usage during compilation
ENV CARGO_BUILD_JOBS=2

RUN cargo build --release -p myc

# Pre-download the fastembed model so it is baked into the image.
# `myc doctor --download` triggers Embedder::new() which downloads the model.
ENV FASTEMBED_CACHE_DIR=/model-cache
RUN mkdir -p /root/.myceliums && ./target/release/myc doctor --download

# ── Stage 2: Runtime ─────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 libgcc-s1 libstdc++6 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/target/release/myc /usr/local/bin/myc
COPY --from=builder /model-cache /model-cache

ENV FASTEMBED_CACHE_DIR=/model-cache

WORKDIR /code
ENTRYPOINT ["myc"]
