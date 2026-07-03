-- EXPLAIN (ANALYZE, BUFFERS) suite for the four inbox hot paths, run for one
-- subscriber. The SQL is copied verbatim from server/src/api/inbox.rs; if a
-- query there changes, this file must be updated to match.
--
--   psql "$DATABASE_URL" -X -v ON_ERROR_STOP=1 -v subscriber=usr_1 -f explain.sql
--
-- Queries run through PREPARE/EXECUTE, matching the prepared-statement path
-- sqlx uses. A single EXECUTE gets a custom plan, which is what the server
-- sees for the first five executions; a forced generic-plan variant of the
-- list query is included because after five executions the plancache may
-- switch to it, changing partition pruning from plan time to run time.
--
-- Paths:
--   a  merged inbox list (direct UNION ALL broadcast, keyset, first + second page)
--   b  unread/unseen counts (maintained counter + broadcast terms)
--   c  mark-all-read watermark upsert + bounded exception GC (rolled back)
--   d  the broadcast fan-out-on-read arm in isolation

\if :{?subscriber}
\else
\set subscriber usr_1
\endif

\set ON_ERROR_STOP on
SET timezone = 'UTC';
DEALLOCATE ALL;

SELECT s.id AS sub, s.environment_id AS env, s.created_at AS sub_created
FROM subscribers s
WHERE s.subscriber_id = :'subscriber' \gset

\echo ''
\echo '================================================================='
\echo 'subscriber' :subscriber
\echo '================================================================='

SELECT :'subscriber' AS subscriber,
       (SELECT count(*) FROM notifications
         WHERE environment_id = :'env' AND subscriber_id = :'sub') AS direct_rows,
       c.unread_direct_count, c.read_watermark, c.archive_watermark
FROM subscriber_counters c
WHERE c.environment_id = :'env' AND c.subscriber_id = :'sub';

-- ============================================================================
-- (a) merged inbox list  [inbox.rs list_items_for]
-- ============================================================================

PREPARE inbox_list (uuid, uuid, timestamptz, uuid, bigint, timestamptz, text) AS
SELECT merged.source AS "source!", merged.id AS "id!",
       merged.category AS "category!", merged.payload AS "payload!",
       merged.occurred_at AS "occurred_at!", merged.read AS "read!",
       merged.archived AS "archived!"
FROM (
  (SELECT 'notification' AS source, n.id, n.category, n.payload,
          n.visible_at AS occurred_at,
          (n.read_at IS NOT NULL
             OR (n.unread_at IS NULL AND n.visible_at <= c.read_watermark)) AS read,
          (n.archived_at IS NOT NULL
             OR (n.unarchived_at IS NULL
                 AND n.visible_at <= c.archive_watermark)) AS archived
     FROM notifications n
     JOIN subscriber_counters c
       ON c.environment_id = n.environment_id
      AND c.subscriber_id  = n.subscriber_id
    WHERE n.environment_id = $1 AND n.subscriber_id = $2
      AND n.visible_at <= now()
      AND (n.visible_at, n.id) < ($3, $4)
      AND ($7 <> 'unread' OR (n.read_at IS NULL
             AND (n.unread_at IS NOT NULL OR n.visible_at > c.read_watermark)))
      AND CASE WHEN $7 = 'archived'
               THEN (n.archived_at IS NOT NULL
                     OR (n.unarchived_at IS NULL
                         AND n.visible_at <= c.archive_watermark))
               ELSE NOT (n.archived_at IS NOT NULL
                     OR (n.unarchived_at IS NULL
                         AND n.visible_at <= c.archive_watermark))
          END
      AND NOT EXISTS (SELECT 1 FROM preferences p
            WHERE p.environment_id = n.environment_id
              AND p.subscriber_id  = n.subscriber_id
              AND p.category = n.category AND p.channel = 'in_app'
              AND p.enabled = false)
    ORDER BY n.visible_at DESC, n.id DESC LIMIT $5)
  UNION ALL
  (SELECT 'broadcast', b.id, b.category, b.payload, b.created_at,
          COALESCE(br.read, b.created_at <= c.read_watermark),
          COALESCE(ba.archived, b.created_at <= c.archive_watermark)
     FROM broadcasts b
     JOIN subscriber_counters c
       ON c.environment_id = b.environment_id AND c.subscriber_id = $2
     LEFT JOIN broadcast_reads br
       ON br.environment_id = b.environment_id
      AND br.subscriber_id  = $2
      AND br.broadcast_id   = b.id
     LEFT JOIN broadcast_archives ba
       ON ba.environment_id = b.environment_id
      AND ba.subscriber_id  = $2
      AND ba.broadcast_id   = b.id
    WHERE b.environment_id = $1
      AND b.created_at >= $6
      AND (b.created_at, b.id) < ($3, $4)
      AND ($7 <> 'unread'
             OR NOT COALESCE(br.read, b.created_at <= c.read_watermark))
      AND CASE WHEN $7 = 'archived'
               THEN COALESCE(ba.archived, b.created_at <= c.archive_watermark)
               ELSE NOT COALESCE(ba.archived, b.created_at <= c.archive_watermark)
          END
      AND NOT EXISTS (SELECT 1 FROM preferences p
            WHERE p.environment_id = b.environment_id
              AND p.subscriber_id  = $2
              AND p.category = b.category AND p.channel = 'in_app'
              AND p.enabled = false)
    ORDER BY b.created_at DESC, b.id DESC LIMIT $5)
) merged
ORDER BY occurred_at DESC, id DESC
LIMIT $5;

\echo ''
\echo '--- (a) merged list, first page, default filter ---'
EXPLAIN (ANALYZE, BUFFERS)
EXECUTE inbox_list(:'env', :'sub', 'infinity',
                   'ffffffff-ffff-ffff-ffff-ffffffffffff', 20, :'sub_created',
                   'default');

-- Keyset cursor for page two = the (occurred_at, id) of page one's last row.
CREATE TEMP TABLE bench_page1 AS
EXECUTE inbox_list(:'env', :'sub', 'infinity',
                   'ffffffff-ffff-ffff-ffff-ffffffffffff', 20, :'sub_created',
                   'default');
SELECT "occurred_at!" AS cur_ts, "id!" AS cur_id
FROM bench_page1 ORDER BY "occurred_at!" ASC, "id!" ASC LIMIT 1 \gset
DROP TABLE bench_page1;

\echo ''
\echo '--- (a) merged list, second page (keyset cursor), default filter ---'
EXPLAIN (ANALYZE, BUFFERS)
EXECUTE inbox_list(:'env', :'sub', :'cur_ts', :'cur_id', 20, :'sub_created',
                   'default');

\echo ''
\echo '--- (a) merged list, first page, unread filter ---'
EXPLAIN (ANALYZE, BUFFERS)
EXECUTE inbox_list(:'env', :'sub', 'infinity',
                   'ffffffff-ffff-ffff-ffff-ffffffffffff', 20, :'sub_created',
                   'unread');

\echo ''
\echo '--- (a) merged list, first page, forced generic plan ---'
SET plan_cache_mode = force_generic_plan;
EXPLAIN (ANALYZE, BUFFERS)
EXECUTE inbox_list(:'env', :'sub', 'infinity',
                   'ffffffff-ffff-ffff-ffff-ffffffffffff', 20, :'sub_created',
                   'default');
RESET plan_cache_mode;

-- ============================================================================
-- (b) unread/unseen counts  [inbox.rs fetch_counts_for]
-- ============================================================================

PREPARE inbox_counts (uuid, uuid, timestamptz) AS
SELECT
    greatest(0, c.unread_direct_count
      + (SELECT count(*) FROM broadcasts b
          WHERE b.environment_id = $1
            AND b.created_at >= $3
            AND b.created_at >  c.read_watermark
            AND NOT EXISTS (SELECT 1 FROM broadcast_reads br
                  WHERE br.environment_id = b.environment_id
                    AND br.subscriber_id  = $2
                    AND br.broadcast_id   = b.id
                    AND br.read)
            AND NOT COALESCE((SELECT ba.archived FROM broadcast_archives ba
                  WHERE ba.environment_id = b.environment_id
                    AND ba.subscriber_id  = $2
                    AND ba.broadcast_id   = b.id),
                  b.created_at <= c.archive_watermark)
            AND NOT EXISTS (SELECT 1 FROM preferences p
                  WHERE p.environment_id = b.environment_id
                    AND p.subscriber_id  = $2
                    AND p.category = b.category AND p.channel = 'in_app'
                    AND p.enabled = false))
      + (SELECT count(*) FROM broadcast_reads br
          JOIN broadcasts b
            ON b.environment_id = br.environment_id AND b.id = br.broadcast_id
          WHERE br.environment_id = $1
            AND br.subscriber_id  = $2
            AND NOT br.read
            AND br.broadcast_created_at <= c.read_watermark
            AND b.created_at >= $3
            AND NOT COALESCE((SELECT ba.archived FROM broadcast_archives ba
                  WHERE ba.environment_id = b.environment_id
                    AND ba.subscriber_id  = $2
                    AND ba.broadcast_id   = b.id),
                  b.created_at <= c.archive_watermark)
            AND NOT EXISTS (SELECT 1 FROM preferences p
                  WHERE p.environment_id = b.environment_id
                    AND p.subscriber_id  = $2
                    AND p.category = b.category AND p.channel = 'in_app'
                    AND p.enabled = false)))::int AS "unread!",
    greatest(0, c.unseen_direct_count
      + (SELECT count(*) FROM broadcasts b
          WHERE b.environment_id = $1
            AND b.created_at >= $3
            AND b.created_at >  c.seen_watermark
            AND NOT EXISTS (SELECT 1 FROM preferences p
                  WHERE p.environment_id = b.environment_id
                    AND p.subscriber_id  = $2
                    AND p.category = b.category AND p.channel = 'in_app'
                    AND p.enabled = false)))::int AS "unseen!"
FROM subscriber_counters c
WHERE c.environment_id = $1 AND c.subscriber_id = $2;

\echo ''
\echo '--- (b) unread/unseen counts ---'
EXPLAIN (ANALYZE, BUFFERS)
EXECUTE inbox_counts(:'env', :'sub', :'sub_created');

-- ============================================================================
-- (c) mark-all-read: watermark upsert + exception GC  [inbox.rs mark_all_read]
-- Runs the real writes in a transaction and rolls back, so the suite is
-- rerunnable. Statement order and locking mirror the handler.
-- ============================================================================

\echo ''
\echo '--- (c) mark-all-read (BEGIN ... ROLLBACK) ---'
BEGIN;

SELECT read_watermark FROM subscriber_counters
 WHERE environment_id = :'env' AND subscriber_id = :'sub' FOR UPDATE;

\echo '--- (c1) watermark upsert ---'
EXPLAIN (ANALYZE, BUFFERS, WAL)
UPDATE subscriber_counters SET
       read_watermark = clock_timestamp(),
       unread_direct_count = 0,
       updated_at = clock_timestamp()
 WHERE environment_id = :'env' AND subscriber_id = :'sub'
 RETURNING read_watermark;

SELECT read_watermark AS wm FROM subscriber_counters
 WHERE environment_id = :'env' AND subscriber_id = :'sub' \gset

\echo '--- (c2) broadcast_reads exception GC ---'
EXPLAIN (ANALYZE, BUFFERS, WAL)
DELETE FROM broadcast_reads
 WHERE environment_id = :'env' AND subscriber_id = :'sub'
   AND broadcast_created_at <= :'wm';

\echo '--- (c3) direct unread-override GC ---'
EXPLAIN (ANALYZE, BUFFERS, WAL)
UPDATE notifications SET unread_at = NULL
 WHERE environment_id = :'env' AND subscriber_id = :'sub'
   AND unread_at IS NOT NULL AND visible_at <= :'wm';

ROLLBACK;

-- ============================================================================
-- (d) the broadcast fan-out-on-read arm in isolation  [inbox.rs
-- list_items_for, second UNION ALL arm]
-- ============================================================================

PREPARE broadcast_arm (uuid, uuid, timestamptz, uuid, bigint, timestamptz, text) AS
SELECT 'broadcast', b.id, b.category, b.payload, b.created_at,
       COALESCE(br.read, b.created_at <= c.read_watermark),
       COALESCE(ba.archived, b.created_at <= c.archive_watermark)
  FROM broadcasts b
  JOIN subscriber_counters c
    ON c.environment_id = b.environment_id AND c.subscriber_id = $2
  LEFT JOIN broadcast_reads br
    ON br.environment_id = b.environment_id
   AND br.subscriber_id  = $2
   AND br.broadcast_id   = b.id
  LEFT JOIN broadcast_archives ba
    ON ba.environment_id = b.environment_id
   AND ba.subscriber_id  = $2
   AND ba.broadcast_id   = b.id
 WHERE b.environment_id = $1
   AND b.created_at >= $6
   AND (b.created_at, b.id) < ($3, $4)
   AND ($7 <> 'unread'
          OR NOT COALESCE(br.read, b.created_at <= c.read_watermark))
   AND CASE WHEN $7 = 'archived'
            THEN COALESCE(ba.archived, b.created_at <= c.archive_watermark)
            ELSE NOT COALESCE(ba.archived, b.created_at <= c.archive_watermark)
       END
   AND NOT EXISTS (SELECT 1 FROM preferences p
         WHERE p.environment_id = b.environment_id
           AND p.subscriber_id  = $2
           AND p.category = b.category AND p.channel = 'in_app'
           AND p.enabled = false)
 ORDER BY b.created_at DESC, b.id DESC LIMIT $5;

\echo ''
\echo '--- (d) broadcast arm, first page, default filter ---'
EXPLAIN (ANALYZE, BUFFERS)
EXECUTE broadcast_arm(:'env', :'sub', 'infinity',
                      'ffffffff-ffff-ffff-ffff-ffffffffffff', 20, :'sub_created',
                      'default');

\echo ''
\echo '--- (d) broadcast arm, unread filter ---'
EXPLAIN (ANALYZE, BUFFERS)
EXECUTE broadcast_arm(:'env', :'sub', 'infinity',
                      'ffffffff-ffff-ffff-ffff-ffffffffffff', 20, :'sub_created',
                      'unread');

DEALLOCATE ALL;
