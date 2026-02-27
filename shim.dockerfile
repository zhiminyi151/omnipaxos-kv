FROM rust:1.84 AS builder

RUN set -eux
ENV CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse

WORKDIR /app
COPY . .

RUN cargo build --release --bin shim

FROM debian:bookworm-slim AS runtime
WORKDIR /app
COPY --from=builder /app/target/release/shim /usr/local/bin
EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/shim"]
