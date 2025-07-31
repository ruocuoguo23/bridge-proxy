FROM rust:1.87 AS builder

WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/network-bridge-proxy /usr/local/bin/network-bridge-proxy

EXPOSE 8080

CMD ["network-bridge-proxy"]