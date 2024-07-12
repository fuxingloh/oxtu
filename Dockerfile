FROM rust:1.79.0-bookworm as builder
RUN apt-get update -y && \
  apt-get install -y clang

WORKDIR /app

COPY Cargo.toml Cargo.lock LICENSE ./
COPY crates ./crates

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --bin oxtu --release && \
    mv /app/target/release/oxtu .

FROM debian:bookworm-slim
RUN apt-get update -y && \
  apt-get install -y openssl

COPY --from=builder /app/oxtu /usr/local/bin

ENV RUST_BACKTRACE=1
ENV RUST_LOG=info
ENV OXTU_PORT=3000
ENV OXTU_LISTEN=0.0.0.0

EXPOSE 3000
CMD ["oxtu"]
