# DB hot-path EXPLAIN harness

Rerunnable EXPLAIN (ANALYZE, BUFFERS) coverage for the four inbox hot paths,
with the SQL copied verbatim from `server/src/api/inbox.rs`:

| Path | Query | Source |
| ---- | ----- | ------ |
| a | merged inbox list (direct UNION ALL broadcast, keyset-paginated) | `list_items_for` |
| b | unread/unseen counts (maintained counter + broadcast terms) | `fetch_counts_for` |
| c | mark-all-read watermark upsert + bounded exception GC | `mark_all_read` |
| d | broadcast fan-out-on-read arm in isolation | `list_items_for`, second arm |

Plain SQL + bash, no toolchain beyond docker and (optionally) psql.

## One-shot run

```bash
bash bench/db/run.sh
```

Boots `postgres:16-alpine` as container `chimely-perf-pg` on host port 5461,
applies `server/migrations/*.sql` in order, seeds at scale 1, runs the suite
for the hot subscriber (`usr_1`) and a median subscriber, then removes the
container. Overridables: `PORT`, `CONTAINER`, `SCALE` (positive integer),
`KEEP=1` to leave the container up, `HOT`/`MEDIAN` to pick subjects.

## Against any Postgres

Any Postgres 15+ with the server migrations applied works. Migrations can be
applied either by booting the server binary once or manually:

```bash
docker run --rm -d --name chimely-perf-pg -e POSTGRES_PASSWORD=pw \
  -p 127.0.0.1:5461:5432 postgres:16-alpine \
  -c shared_buffers=512MB -c track_io_timing=on -c max_wal_size=4GB
export DATABASE_URL=postgres://postgres:pw@127.0.0.1:5461/postgres

for f in server/migrations/*.sql; do
  psql "$DATABASE_URL" -X -v ON_ERROR_STOP=1 -1 -f "$f"
done

psql "$DATABASE_URL" -X -v ON_ERROR_STOP=1 -v scale=1 -f bench/db/seed.sql
psql "$DATABASE_URL" -X -v ON_ERROR_STOP=1 -v subscriber=usr_1     -f bench/db/explain.sql
psql "$DATABASE_URL" -X -v ON_ERROR_STOP=1 -v subscriber=usr_25000 -f bench/db/explain.sql

docker rm -f chimely-perf-pg
```

## Dataset (scale 1)

- 4 environments, 50k subscribers, 3M notifications, 2k broadcasts.
- `notifications` monthly partitions are created by the seed for the last 13
  months plus next month, named exactly as the server maintenance job names
  them, so seeding a server-booted database is safe.
- `usr_1` (environment 1) is the hot subscriber: 1% of all notifications
  (~30k rows), read watermark 45 days back, 150 explicit broadcast reads
  straddling it (roughly 60 above, the rest at or below, so the GC has real
  rows to delete). Every other subscriber gets a uniform share (~60 rows),
  so any `usr_N` is a median subscriber.
- Read/unread/archived mix, per-item override rows, broadcast
  read/archive exceptions, category mutes, and per-subscriber watermarks are
  seeded; maintained counters are recomputed from the rows afterwards
  (mute-aware terms omitted, values are plan-shape-accurate only).
- `seed.sql` truncates all inbox tables first: rerunning reseeds from
  scratch. Ids are UUIDv7 with timestamps matching the ordering column, as
  the app mints them.

## Reading the output

- Queries run via PREPARE/EXECUTE, matching sqlx's prepared statements. One
  EXECUTE yields a custom plan (what the server gets for the first five
  executions). The list also runs under `plan_cache_mode =
  force_generic_plan` because the plancache may switch to a generic plan
  later, which moves partition pruning from plan time to run time.
- Path c executes its writes for real inside BEGIN ... ROLLBACK, so the
  suite stays rerunnable. WAL stats are included for the write statements.
- Page one of path a is executed once untimed to derive the page-two keyset
  cursor, so page-two buffer counts reflect a warm cache.

## Known scaling term

The broadcast arm under the unread filter is the only query that cannot
stop early. Every broadcast in the visibility window is fetched and joined
against the subscriber's read exceptions, so cost grows linearly with the
window (one exception probe per window broadcast; a plan that materializes
the exception list instead pays the full product). Measured 11.4 ms at 500
window broadcasts x 152 exceptions, all cache hits. Both factors are structurally
bounded: mark-all-read GC empties the exception list, and the window only
grows with retained broadcasts. Environments that accumulate tens of
thousands of visible broadcasts will degrade linearly on unread-filtered
pages before any other path moves.
