-- Seed for the DB hot-path EXPLAIN harness. Set-based and rerunnable: it
-- truncates every inbox table first, then rebuilds the dataset at the
-- requested scale. Requires the server migrations to be applied already.
--
--   psql "$DATABASE_URL" -X -v ON_ERROR_STOP=1 -v scale=1 -f seed.sql
--
-- scale 1 = 4 environments, 50k subscribers, 3M notifications, 2k broadcasts,
-- 100k broadcast read exceptions, 20k broadcast archive exceptions. scale
-- must be a positive integer and multiplies everything except environments.
--
-- Shape choices, mirrored from production expectations:
--   * usr_1 (environment 1) is the hot subscriber: 1% of all notifications.
--     Every other notification lands on a uniformly random subscriber, so any
--     other usr_N is a median subscriber.
--   * visible_at spreads over the last 12 months; 1% of rows are scheduled
--     in the future (deliver_at set, visible_at > now()).
--   * ~55% of visible rows are read via read_at, ~0.5% of unread rows carry
--     an unread_at override, ~10% are archived, ~0.3% carry unarchived_at.
--   * watermarks are per subscriber: 30% never marked all read (epoch), the
--     rest within the last 180 days.
--   * counters are recomputed from the seeded rows (mute-aware terms
--     omitted, close enough for plan shape).

\if :{?scale}
\else
\set scale 1
\endif

\set ON_ERROR_STOP on
\timing on

SET timezone = 'UTC';
SELECT setseed(0.42);

-- ============================================================================
-- Helpers
-- ============================================================================

-- UUIDv7 with the millisecond timestamp taken from ts, so ids correlate with
-- their ordering column the way app-minted UUIDv7 ids do.
CREATE OR REPLACE FUNCTION bench_uuid_v7(ts timestamptz) RETURNS uuid
LANGUAGE sql VOLATILE AS $$
  SELECT encode(
    set_bit(
      set_bit(
        overlay(uuid_send(gen_random_uuid())
                placing substring(int8send((extract(epoch FROM ts) * 1000)::bigint) FROM 3)
                FROM 1 FOR 6),
        52, 1),
      53, 1),
    'hex')::uuid
$$;

-- ============================================================================
-- Reset
-- ============================================================================

TRUNCATE notifications, notification_status_log, broadcast_reads,
         broadcast_archives, broadcasts, preferences, subscriber_counters,
         subscribers, jobs, dead_letters, idempotency_keys, api_keys,
         environments CASCADE;

-- Monthly partitions covering the seeded range, named exactly as the server
-- partition maintenance job names them so a server-migrated database is
-- compatible (IF NOT EXISTS dedupes).
DO $$
DECLARE m date;
BEGIN
  FOR m IN SELECT generate_series(date_trunc('month', now()) - interval '13 months',
                                  date_trunc('month', now()) + interval '1 month',
                                  interval '1 month')::date
  LOOP
    EXECUTE format(
      'CREATE TABLE IF NOT EXISTS notifications_%s PARTITION OF notifications
       FOR VALUES FROM (%L) TO (%L)',
      to_char(m, 'YYYY_MM'), m, m + interval '1 month');
  END LOOP;
END $$;

-- ============================================================================
-- Environments (4, fixed ids so explain runs are deterministic)
-- ============================================================================

INSERT INTO environments (id, slug, name, subscriber_hmac_secret)
SELECT ('00000000-0000-7000-8000-00000000000' || i)::uuid,
       'bench-env-' || i,
       'Bench env ' || i,
       'bench-secret'
FROM generate_series(1, 4) i;

-- ============================================================================
-- Subscribers: usr_N maps to environment ((N-1) % 4) + 1. created_at is
-- backdated up to 540 days (broadcast visibility varies per subscriber).
-- usr_1 is pinned oldest so the hot subscriber sees every broadcast.
-- ============================================================================

INSERT INTO subscribers (environment_id, id, subscriber_id, created_at, updated_at)
SELECT env_id, bench_uuid_v7(created_at), 'usr_' || g, created_at, created_at
FROM (
  SELECT g,
         ('00000000-0000-7000-8000-00000000000' || ((g - 1) % 4 + 1))::uuid AS env_id,
         CASE WHEN g = 1 THEN now() - interval '540 days'
              ELSE now() - random() * interval '540 days' END AS created_at
  FROM generate_series(1, 50000 * :scale) g
) s;

-- ============================================================================
-- Counters: watermarks first, unread/unseen recomputed after the fact.
-- ============================================================================

INSERT INTO subscriber_counters
    (environment_id, subscriber_id, unread_direct_count, unseen_direct_count,
     read_watermark, seen_watermark, archive_watermark, updated_at)
SELECT s.environment_id, s.id, 0, 0,
       CASE WHEN random() < 0.3 THEN 'epoch'::timestamptz
            ELSE now() - random() * interval '180 days' END,
       CASE WHEN random() < 0.3 THEN 'epoch'::timestamptz
            ELSE now() - random() * interval '120 days' END,
       CASE WHEN random() < 0.9 THEN 'epoch'::timestamptz
            ELSE now() - random() * interval '90 days' END,
       now()
FROM subscribers s;

-- The hot subscriber has marked all read 45 days ago: above-watermark unread
-- rows plus below-watermark exception rows, the worst realistic mix.
UPDATE subscriber_counters c SET
    read_watermark = now() - interval '45 days',
    seen_watermark = now() - interval '30 days'
FROM subscribers s
WHERE s.environment_id = c.environment_id AND s.id = c.subscriber_id
  AND s.subscriber_id = 'usr_1';

-- ============================================================================
-- Notifications: 3M * scale rows. Every 100th row goes to usr_1.
-- ============================================================================

INSERT INTO notifications
    (environment_id, id, subscriber_id, category, payload,
     created_at, deliver_at, visible_at,
     read_at, unread_at, archived_at, unarchived_at)
SELECT
    s.environment_id,
    bench_uuid_v7(t.visible_at),
    s.id,
    v.category,
    jsonb_build_object('title', 'n-' || v.g, 'body', 'bench payload body'),
    CASE WHEN v.future THEN t.visible_at - interval '1 day' ELSE t.visible_at END,
    CASE WHEN v.future THEN t.visible_at END,
    t.visible_at,
    CASE WHEN NOT v.future AND v.r_read < 0.55
         THEN t.visible_at + interval '1 hour' END,
    CASE WHEN NOT v.future AND v.r_read >= 0.55 AND v.r_unread < 0.01
         THEN t.visible_at + interval '2 hours' END,
    CASE WHEN NOT v.future AND v.r_arch < 0.10
         THEN t.visible_at + interval '3 hours' END,
    CASE WHEN NOT v.future AND v.r_arch >= 0.10 AND v.r_arch < 0.103
         THEN t.visible_at + interval '3 hours' END
FROM (
  SELECT g,
         CASE WHEN g % 100 = 0 THEN 1
              ELSE 1 + floor(random() * (50000 * :scale))::int END AS sub_num,
         random() < 0.01 AS future,
         random() AS r_read,
         random() AS r_unread,
         random() AS r_arch,
         (array['billing','security','social','product','digest'])
             [1 + floor(random() * 5)::int] AS category,
         random() AS r_vis
  FROM generate_series(1, 3000000 * :scale) g
) v
JOIN subscribers s ON s.subscriber_id = 'usr_' || v.sub_num
CROSS JOIN LATERAL (
  SELECT CASE WHEN v.future THEN now() + v.r_vis * interval '20 days'
              ELSE now() - v.r_vis * interval '360 days' END AS visible_at
) t (visible_at)
ORDER BY t.visible_at;

-- ============================================================================
-- Broadcasts: 2k * scale across the 4 environments, last 12 months.
-- ============================================================================

INSERT INTO broadcasts (environment_id, id, category, payload, created_at)
SELECT b.env_id, bench_uuid_v7(b.created_at), b.category,
       jsonb_build_object('title', 'b-' || b.g, 'body', 'bench broadcast'),
       b.created_at
FROM (
  SELECT g,
         ('00000000-0000-7000-8000-00000000000' || ((g - 1) % 4 + 1))::uuid AS env_id,
         now() - random() * interval '360 days' AS created_at,
         (array['billing','security','social','product','digest'])
             [1 + floor(random() * 5)::int] AS category
  FROM generate_series(1, 2000 * :scale) g
) b;

-- ============================================================================
-- Broadcast exception rows: random (subscriber, broadcast) pairs within the
-- subscriber's environment. 90% explicit reads, 10% unread overrides.
-- ============================================================================

WITH numbered AS (
  SELECT environment_id, id, created_at,
         row_number() OVER (PARTITION BY environment_id ORDER BY id) AS rn
  FROM broadcasts
), per_env AS (
  SELECT environment_id, count(*) AS c FROM broadcasts GROUP BY 1
)
INSERT INTO broadcast_reads
    (environment_id, subscriber_id, broadcast_id, broadcast_created_at, read, read_at)
SELECT s.environment_id, s.id, b.id, b.created_at,
       random() < 0.9, b.created_at + interval '1 hour'
FROM (
  SELECT 1 + floor(random() * (50000 * :scale))::int AS sub_num,
         random() AS rb
  FROM generate_series(1, 100000 * :scale)
) r
JOIN subscribers s ON s.subscriber_id = 'usr_' || r.sub_num
JOIN per_env ce ON ce.environment_id = s.environment_id
JOIN numbered b ON b.environment_id = s.environment_id
               AND b.rn = 1 + floor(r.rb * ce.c)::int
ON CONFLICT DO NOTHING;

-- The hot subscriber read its 150 newest broadcasts individually (all above
-- its watermark), the population the mark-all-read GC exists to collapse.
INSERT INTO broadcast_reads
    (environment_id, subscriber_id, broadcast_id, broadcast_created_at, read, read_at)
SELECT b.environment_id, s.id, b.id, b.created_at, true, b.created_at + interval '1 minute'
FROM subscribers s
CROSS JOIN LATERAL (
  SELECT * FROM broadcasts b
  WHERE b.environment_id = s.environment_id
  ORDER BY b.created_at DESC LIMIT 150
) b
WHERE s.subscriber_id = 'usr_1'
ON CONFLICT (environment_id, subscriber_id, broadcast_id)
DO UPDATE SET read = true;

WITH numbered AS (
  SELECT environment_id, id, created_at,
         row_number() OVER (PARTITION BY environment_id ORDER BY id) AS rn
  FROM broadcasts
), per_env AS (
  SELECT environment_id, count(*) AS c FROM broadcasts GROUP BY 1
)
INSERT INTO broadcast_archives
    (environment_id, subscriber_id, broadcast_id, broadcast_created_at, archived)
SELECT s.environment_id, s.id, b.id, b.created_at, random() < 0.7
FROM (
  SELECT 1 + floor(random() * (50000 * :scale))::int AS sub_num,
         random() AS rb
  FROM generate_series(1, 20000 * :scale)
) r
JOIN subscribers s ON s.subscriber_id = 'usr_' || r.sub_num
JOIN per_env ce ON ce.environment_id = s.environment_id
JOIN numbered b ON b.environment_id = s.environment_id
               AND b.rn = 1 + floor(r.rb * ce.c)::int
ON CONFLICT DO NOTHING;

-- ============================================================================
-- Preferences: 5% of subscribers mute one category.
-- ============================================================================

INSERT INTO preferences (environment_id, subscriber_id, category, channel, enabled)
SELECT s.environment_id, s.id,
       (array['billing','security','social','product','digest'])
           [1 + floor(random() * 5)::int],
       'in_app', false
FROM subscribers s
WHERE random() < 0.05
ON CONFLICT DO NOTHING;

-- ============================================================================
-- Recompute maintained counters from the seeded rows. The mute-aware term is
-- omitted on purpose: it changes counter values, not plan shapes.
-- ============================================================================

UPDATE subscriber_counters c SET
    unread_direct_count = a.unread,
    unseen_direct_count = a.unseen,
    updated_at = now()
FROM (
  SELECT n.environment_id, n.subscriber_id,
         count(*) FILTER (WHERE n.visible_at <= now()
           AND NOT (n.read_at IS NOT NULL
                    OR (n.unread_at IS NULL AND n.visible_at <= c2.read_watermark))
           AND NOT (n.archived_at IS NOT NULL
                    OR (n.unarchived_at IS NULL AND n.visible_at <= c2.archive_watermark))
         )::int AS unread,
         count(*) FILTER (WHERE n.visible_at <= now()
           AND n.visible_at > c2.seen_watermark)::int AS unseen
  FROM notifications n
  JOIN subscriber_counters c2
    ON c2.environment_id = n.environment_id AND c2.subscriber_id = n.subscriber_id
  GROUP BY 1, 2
) a
WHERE c.environment_id = a.environment_id AND c.subscriber_id = a.subscriber_id;

VACUUM ANALYZE notifications, broadcasts, broadcast_reads, broadcast_archives,
               subscribers, subscriber_counters, preferences;

SELECT (SELECT count(*) FROM subscribers)     AS subscribers,
       (SELECT count(*) FROM notifications)   AS notifications,
       (SELECT count(*) FROM broadcasts)      AS broadcasts,
       (SELECT count(*) FROM broadcast_reads) AS broadcast_reads;
