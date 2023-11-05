FROM rust:1-alpine3.16 AS builder

ENV RUSTFLAGS="-C target-feature=-crt-static"
RUN apk add --no-cache musl-dev

WORKDIR /app
COPY ./ /app

COPY ./templates /templates
RUN cargo build --release
RUN strip target/release/isitonline

FROM alpine:3.16

RUN apk add --no-cache libgcc
COPY --from=builder /app/target/release/isitonline .
COPY ./static /static

ENTRYPOINT ["/isitonline"]
