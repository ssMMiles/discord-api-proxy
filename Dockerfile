FROM debian:bullseye-slim AS builder

RUN apt-get update && apt-get upgrade -y && \
    apt-get install gcc libssl-dev pkg-config protobuf-compiler git libssl-dev ca-certificates curl -y

RUN curl https://sh.rustup.rs -sSf | sh -s -- -y

WORKDIR /app

COPY Cargo.toml .
COPY src/ src/

RUN ~/.cargo/bin/cargo build --release && \
    cp target/release/discord-api-proxy /usr/bin/discord-api-proxy && \
    ~/.cargo/bin/cargo clean

ENTRYPOINT ["/usr/bin/discord-api-proxy"]