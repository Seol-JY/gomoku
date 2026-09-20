# gomoku-server image, used by `docker compose --profile full up`
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p server --locked

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/gomoku-server /usr/local/bin/gomoku-server
EXPOSE 3000
USER 10001:10001
ENTRYPOINT ["gomoku-server"]
