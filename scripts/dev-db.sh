#!/usr/bin/env sh
# Starts the local Postgres via docker compose and waits until it accepts
# connections. Usage: scripts/dev-db.sh [up|down|reset|psql]
set -eu
cd "$(dirname "$0")/.."
cmd="${1:-up}"
case "$cmd" in
  up)
    docker compose up -d db
    printf 'waiting for postgres'
    for _ in $(seq 1 30); do
      if docker compose exec -T db pg_isready -U "${POSTGRES_USER:-gomoku}" >/dev/null 2>&1; then
        echo ' ready'
        exit 0
      fi
      printf '.'
      sleep 1
    done
    echo ' timed out' >&2
    exit 1
    ;;
  down)  docker compose down ;;
  reset) docker compose down -v && exec "$0" up ;;
  psql)  exec docker compose exec db psql -U "${POSTGRES_USER:-gomoku}" -d "${POSTGRES_DB:-gomoku}" ;;
  *) echo "usage: $0 [up|down|reset|psql]" >&2; exit 2 ;;
esac
