# This dockerfile is designed for use in CI to build
# release artifacts. For testing, you should be able 
# to get away with a normal `cargo build --release`.

# Matches the version of Ubuntu used by the Actions runners.
FROM ubuntu:20.04

# This stuff almost never updates
ARG TARGET_TRIPLE
ENV DISCORD_WEBHOOK="" LISTEN_ADDRESS=0.0.0.0:9094 RUST_LOG=info DEBIAN_FRONTEND=noninteractive
LABEL org.opencontainers.image.source="https://github.com/SriRamanujam/alertmanager-discord-bridge"

# This doesn't update quite as often
COPY COPYING /COPYING
CMD ["/alertmanager-discord-bridge"]

# Updates a lot
RUN apt-get update && apt-get upgrade -y && apt-get install -y libssl-dev ca-certificates && rm -rf /var/lib/apt/lists/*
COPY target/$TARGET_TRIPLE/release/alertmanager-discord-bridge /alertmanager-discord-bridge
