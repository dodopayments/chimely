#!/usr/bin/env bash
# One-shot runner: boots a throwaway Postgres container, applies the server
# migrations, seeds at SCALE, and runs the EXPLAIN suite for a hot and a
# median subscriber. Removes the container on exit unless KEEP=1.
#
#   SCALE=1 PORT=5461 bash bench/db/run.sh
#
# Requires docker. Uses host psql when present, docker exec otherwise.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MIGRATIONS_DIR="$SCRIPT_DIR/../../server/migrations"

CONTAINER="${CONTAINER:-chimely-perf-pg}"
PORT="${PORT:-5461}"
SCALE="${SCALE:-1}"
KEEP="${KEEP:-0}"
HOT="${HOT:-usr_1}"
# Any non-hot subscriber holds a median inbox by construction (uniform spread).
MEDIAN="${MEDIAN:-usr_$((SCALE * 25000))}"
DATABASE_URL="postgres://postgres:pw@127.0.0.1:${PORT}/postgres"

psql_run() {
  if command -v psql >/dev/null 2>&1; then
    psql "$DATABASE_URL" -X -q -v ON_ERROR_STOP=1 "$@"
  else
    docker exec -i "$CONTAINER" psql -U postgres -X -q -v ON_ERROR_STOP=1 "$@"
  fi
}

cleanup() {
  if [ "$KEEP" != "1" ]; then
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
docker run -d --name "$CONTAINER" \
  -e POSTGRES_PASSWORD=pw \
  -p "127.0.0.1:${PORT}:5432" \
  postgres:16-alpine \
  -c shared_buffers=512MB -c track_io_timing=on -c max_wal_size=4GB >/dev/null

echo "waiting for postgres on port ${PORT}"
for _ in $(seq 1 60); do
  if docker exec "$CONTAINER" pg_isready -U postgres >/dev/null 2>&1; then break; fi
  sleep 1
done
docker exec "$CONTAINER" pg_isready -U postgres >/dev/null

echo "applying migrations"
for f in "$MIGRATIONS_DIR"/*.sql; do
  echo "  $(basename "$f")"
  psql_run -1 -f /dev/stdin <"$f"
done

echo "seeding (scale=${SCALE})"
time psql_run -v scale="$SCALE" -f /dev/stdin <"$SCRIPT_DIR/seed.sql"

for sub in "$HOT" "$MEDIAN"; do
  echo "explain suite for ${sub}"
  psql_run -v subscriber="$sub" -f /dev/stdin <"$SCRIPT_DIR/explain.sql"
done
