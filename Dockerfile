FROM rust:1-alpine3.18 AS builder

ENV RUSTFLAGS="-C target-feature=-crt-static"
ENV DATABASE_URL="sqlite:/app/db/data.db"
RUN apk add --no-cache musl-dev pkgconfig openssl libressl-dev
RUN cargo install sqlx-cli

WORKDIR /app
COPY Cargo.toml /app/
COPY ./src /app/src
COPY ./migrations /app/migrations
COPY ./templates /app/templates

RUN mkdir db
RUN sqlx database create
RUN sqlx migrate run

RUN cargo build --release
RUN strip target/release/isitonline

FROM alpine:3.18

RUN apk add --no-cache libgcc libressl-dev
COPY --from=builder /app/target/release/isitonline .
COPY --from=builder /app/db /db
COPY ./static /static

ENV IP="0.0.0.0"
ENTRYPOINT ["/isitonline"]
