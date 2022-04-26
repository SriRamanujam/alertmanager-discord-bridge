# Alertmanager to Discord Bridge

This is a small program that simply listens for incoming Alertmanager webhook payloads, parses them, and puts them into a Discord server of your choice via a Discord webhook. It is specifically designed to work with the sort of Alertmanager payloads Openshift puts out, though it should be generic to all Alertmanager instances. I will happily accept PRs to improve the flexibility of the JSON payload parsing.

## Usage

Use the Docker image. It's what it's there for.

```sh
docker run -d -p 9094:9094 \
    -e DISCORD_WEBHOOK="<your discord server's webhook here>" \
    ghcr.io/sriramanujam/alertmanager-discord-bridge:latest
```

## Building

You can of course build the project manually. It's Rust, so you'll need the Rust toolchain installed.

```sh
cargo build
```

MSRV: 1.54
