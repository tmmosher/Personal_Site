FROM rust:1.86-slim as build

RUN USER=root cargo new --bin Checkout_Server
WORKDIR /Checkout_Server

COPY ./Cargo.lock ./Cargo.lock
COPY ./Cargo.toml ./Cargo.toml
COPY ./.env ./.env

RUN cargo build --release
RUN rm src/*.rs

COPY ./src ./src

RUN rm ./target/release/deps/Checkout_Server*
RUN cargo build --release

FROM rust:1.86-slim as dev

COPY --from=build /Checkout_Server/target/release/Checkout_Server .

CMD ["./Checkout_Webserver"]