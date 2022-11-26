FROM rust:slim AS builder

RUN apt-get update && apt-get upgrade -y && \
  apt-get install libssl-dev pkg-config protobuf-compiler git -y

WORKDIR /app

FROM builder AS dependencies

COPY Cargo.toml .

RUN mkdir src && \
  echo "fn main() {}" > src/main.rs && \
  cargo build --release

FROM builder as application

COPY --from=dependencies /app/Cargo.toml .
COPY --from=dependencies /usr/local/cargo /usr/local/cargo
COPY --from=dependencies /app/target/release /app/target/release

COPY src/ src/

RUN cargo build --release

FROM debian:bullseye-slim AS runtime

RUN apt-get update && apt-get upgrade -y && \
  apt-get install libssl-dev ca-certificates -y

COPY --from=application /app/target/release/discord-api-proxy /usr/local/bin

ENTRYPOINT ["/usr/local/bin/discord-api-proxy"]