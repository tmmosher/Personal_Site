FROM rust:1.86-slim as build

# openSSL + pkg-config because otherwise this fails
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
      pkg-config \
      libssl-dev \
      build-essential \
      ca-certificates && \
    rm -rf /var/lib/apt/lists/*

RUN USER=root cargo new --bin Checkout_Server
WORKDIR /Checkout_Server

COPY ./Cargo.lock ./Cargo.lock
COPY ./Cargo.toml ./Cargo.toml
COPY ./.env ./.env

RUN cargo build --release
RUN rm src/*.rs

COPY ./src ./src

RUN rm ./target/release/deps/*
RUN cargo build --release

FROM debian:bookworm-slim as dev
RUN apt-get update && apt-get install -y --no-install-recommends libssl-dev && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /Checkout_Server

COPY --from=build /Checkout_Server/target/release/ .
COPY .env .env

CMD ["./Checkout_Webserver"]