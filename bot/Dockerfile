# syntax=docker/dockerfile:1

# ---------------------------------------------------------------------------
# Stage 1 — dependency cache.
# cargo-chef turns the manifests into a "recipe" so the ~200 crates this
# project pulls in are rebuilt only when Cargo.toml changes, not on every edit.
# ---------------------------------------------------------------------------
FROM rust:1-slim-bookworm AS chef

RUN printf 'Acquire::Retries "10";\nAcquire::http::Timeout "60";\n' > /etc/apt/apt.conf.d/99-retries \
    && apt-get update -qq \
    && apt-get install -y --no-install-recommends --fix-missing pkg-config \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked --version ^0.1

WORKDIR /app

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo chef prepare --recipe-path recipe.json

# ---------------------------------------------------------------------------
# Stage 2 — build.
# ---------------------------------------------------------------------------
FROM chef AS builder

COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Queries are runtime-checked, so no database is needed at build time.
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY locales ./locales
COPY migrations ./migrations
RUN cargo build --release --bin zeronsfw-bot

# ---------------------------------------------------------------------------
# Stage 3 — runtime. No toolchain, no source.
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# rustls needs the CA bundle to reach api.telegram.org.
RUN printf 'Acquire::Retries "10";\nAcquire::http::Timeout "60";\n' > /etc/apt/apt.conf.d/99-retries \
    && apt-get update -qq \
    && apt-get install -y --no-install-recommends --fix-missing ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --uid 10001 bot
WORKDIR /app

COPY --from=builder /app/target/release/zeronsfw-bot /usr/local/bin/zeronsfw-bot

USER bot
ENTRYPOINT ["/usr/local/bin/zeronsfw-bot"]
