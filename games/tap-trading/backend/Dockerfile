# Multi-stage build for the four tap-trading service binaries.
#
# One `builder` stage compiles the whole workspace once; each service is a thin
# runtime target selected via `--target` (compose `build.target`). All crates use
# rustls, so no OpenSSL/system TLS libs are needed — `ca-certificates` is enough.
# Migrations are embedded at compile time (`sqlx::migrate!`), so the migrate
# image needs no SQL files on disk.
#
# CLI deps: BOTH the worker (settlement) and the api (withdraw) shell out to the
# `sui` CLI, so they share `cli-base`. The worker additionally needs `walrus`.

# ---- builder: compiles every service binary from the workspace ----
FROM rust:1-slim-bookworm AS builder
WORKDIR /build
RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config \
 && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --release \
      --bin tap-trading-api \
      --bin tap-trading-oracle-aggregator \
      --bin tap-trading-settlement-worker \
      --bin tap-trading-migrate

# ---- runtime base shared by every service ----
# Ubuntu 24.04 (not debian-slim) because the prebuilt `sui`/`walrus` binaries are
# linked against Ubuntu 24.04's glibc 2.39 + libstdc++; bookworm's glibc 2.36 is
# too old and `sui` fails to load. Rust bins built on the bookworm builder run
# fine here (glibc is backward-compatible). `curl` is kept for healthchecks.
FROM ubuntu:24.04 AS runtime-base
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /app

FROM runtime-base AS aggregator
COPY --from=builder /build/target/release/tap-trading-oracle-aggregator /usr/local/bin/
EXPOSE 8081
ENTRYPOINT ["tap-trading-oracle-aggregator"]

FROM runtime-base AS migrate
COPY --from=builder /build/target/release/tap-trading-migrate /usr/local/bin/
ENTRYPOINT ["tap-trading-migrate"]

# ---- cli-base: runtime + the `sui` CLI (needed by api withdraw + worker) ----
# Fetched for the build's target arch. SUI_VERSION is overridable; empty resolves
# the latest testnet release tag. `sui --version` fails the build if it's broken.
FROM runtime-base AS cli-base
ARG TARGETARCH
ARG SUI_VERSION=
RUN set -eux; \
    arch="${TARGETARCH:-$(dpkg --print-architecture)}"; \
    case "$arch" in \
      amd64) suiarch=x86_64 ;; \
      arm64) suiarch=aarch64 ;; \
      *) echo "unsupported arch: $arch" >&2; exit 1 ;; \
    esac; \
    suitag="${SUI_VERSION}"; \
    if [ -z "$suitag" ]; then \
      suitag="$(curl -fsSL 'https://api.github.com/repos/MystenLabs/sui/releases?per_page=40' \
        | grep -oE '"tag_name": *"testnet-[^"]+"' | head -n1 \
        | sed -E 's/.*"(testnet-[^"]+)".*/\1/')"; \
    fi; \
    echo "installing sui $suitag ($suiarch)"; \
    curl -fsSL -o /tmp/sui.tgz \
      "https://github.com/MystenLabs/sui/releases/download/${suitag}/sui-${suitag}-ubuntu-${suiarch}.tgz"; \
    ( tar -xzf /tmp/sui.tgz -C /usr/local/bin sui ) || \
    ( mkdir -p /tmp/sui && tar -xzf /tmp/sui.tgz -C /tmp/sui && \
      install -m755 "$(find /tmp/sui -type f -name sui | head -n1)" /usr/local/bin/sui ); \
    rm -rf /tmp/sui*; \
    sui --version

# api signs withdrawals via the sui CLI (ADR-0010) and verifies deposits via RPC.
FROM cli-base AS api
COPY --from=builder /build/target/release/tap-trading-api /usr/local/bin/
EXPOSE 8080
ENTRYPOINT ["tap-trading-api"]

# settlement worker also publishes proofs via the `walrus` CLI.
FROM cli-base AS worker
ARG TARGETARCH
ARG WALRUS_NETWORK=testnet
RUN set -eux; \
    arch="${TARGETARCH:-$(dpkg --print-architecture)}"; \
    case "$arch" in \
      amd64) walarch=x86_64 ;; \
      arm64) walarch=aarch64 ;; \
      *) echo "unsupported arch: $arch" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /usr/local/bin/walrus \
      "https://storage.googleapis.com/mysten-walrus-binaries/walrus-${WALRUS_NETWORK}-latest-ubuntu-${walarch}"; \
    chmod +x /usr/local/bin/walrus; \
    walrus --version
COPY --from=builder /build/target/release/tap-trading-settlement-worker /usr/local/bin/
ENTRYPOINT ["tap-trading-settlement-worker"]
