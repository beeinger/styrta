# Dependency layers stay cached until Cargo.lock changes.
#   docker build -t styrta-serve:dev -f backend/Dockerfile backend

FROM rust:1.95-bookworm AS chef
RUN cargo install cargo-chef --locked --version 0.1.73
WORKDIR /app

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json --bin serve --bin styrta
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
RUN cargo build --release --bin serve --bin styrta

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 poppler-utils \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /var/lib/styrta/audio
WORKDIR /app
COPY --from=builder /app/migrations /app/migrations
COPY --from=builder /app/target/release/serve /usr/local/bin/serve
COPY --from=builder /app/target/release/styrta /usr/local/bin/styrta
EXPOSE 8088
ENTRYPOINT ["/usr/local/bin/serve"]
