# ── Stage 1: dependency cache ──────────────────────────────────────────────
# Build a dummy project first so Cargo's dependency layer is cached separately
# from the application source. Subsequent builds only recompile your code.
FROM rust:1.82-slim AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY src src/
RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 2: build ─────────────────────────────────────────────────────────
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY Cargo.toml Cargo.lock ./
COPY src src/
COPY migrations migrations/
RUN cargo build --release --bins

# ── Stage 3a: API runtime image ────────────────────────────────────────────
FROM debian:bookworm-slim AS api
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/career-watch-api /usr/local/bin/career-watch-api
EXPOSE 8080
CMD ["career-watch-api"]

# ── Stage 3b: worker runtime image ─────────────────────────────────────────
FROM debian:bookworm-slim AS worker
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/career-watch-worker /usr/local/bin/career-watch-worker
CMD ["career-watch-worker"]

# ── Stage 3c: discovery worker runtime image ───────────────────────────────
FROM debian:bookworm-slim AS discovery
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/career-watch-discovery /usr/local/bin/career-watch-discovery
CMD ["career-watch-discovery"]
