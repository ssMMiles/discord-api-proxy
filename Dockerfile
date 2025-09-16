FROM rust:slim-trixie AS builder

WORKDIR /app

COPY Cargo.toml .
COPY src/ src/

RUN apt-get update && \
    apt-get install gcc libssl-dev pkg-config protobuf-compiler -y && \
    cargo build --release

FROM debian:trixie-slim

RUN apt-get update && apt-get install libssl-dev ca-certificates -y

COPY --from=builder /app/target/release/discord-api-proxy /usr/bin/discord-api-proxy

ENTRYPOINT ["/usr/bin/discord-api-proxy"]