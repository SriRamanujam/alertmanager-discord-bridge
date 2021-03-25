FROM rust:1.50 AS builder
WORKDIR /code
COPY . .
RUN cargo install --path .

FROM debian:buster-slim
ENV DISCORD_WEBHOOK LISTEN_ADDRESS=0.0.0.0:9094 RUST_LOG=info
RUN apt-get update && apt-get install -y libssl-dev && rm -rf /var/lib/apt/lists/*
COPY --from=builder /code/target/release/alertmanager-discord-bridge /alertmanager-discord-bridge
CMD ["/alertmanager-discord-bridge"]
