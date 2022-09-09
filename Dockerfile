FROM lukemathwalker/cargo-chef:latest-rust-slim AS chef

RUN apt-get update && apt-get upgrade -y
RUN apt-get install libssl-dev pkg-config -y

WORKDIR /app

FROM chef AS planner

COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs

RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json

RUN apt-get install protobuf-compiler -y

# Build dependencies - this is the caching Docker layer!
RUN cargo chef cook --release --recipe-path recipe.json

COPY src src

RUN cargo build --release --bin discord-api-proxy

# We do not need the Rust toolchain to run the binary!
FROM debian:bullseye-slim AS runtime

RUN apt-get update && apt-get upgrade -y
RUN apt-get install libssl-dev ca-certificates -y

FROM runtime as runner

COPY --from=builder /app/target/release/discord-api-proxy /usr/local/bin

ENTRYPOINT ["/usr/local/bin/discord-api-proxy"]