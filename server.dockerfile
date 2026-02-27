FROM rust:1.84 AS builder

# Stop if a command fails
RUN set -eux
ENV CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse

WORKDIR /app
COPY . .

# Build application (no cargo-chef to avoid edition2024 dependency issues)
RUN cargo build --release --bin server

FROM debian:bookworm-slim AS runtime
WORKDIR /app
COPY --from=builder /app/target/release/server /usr/local/bin
EXPOSE 8000
ENTRYPOINT ["/usr/local/bin/server"]
