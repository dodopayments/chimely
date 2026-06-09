-- =============================================================================
-- Dronte v1 — Postgres schema (contract-first spec)
--
-- Deployment model: single-org, multi-consumer. There is no organizations
-- table; environments are the isolation unit. environment_id appears in every
-- PK, every UNIQUE constraint, and every FK — it is what enforces hard
-- isolation between consumer apps, and it preserves optionality for an
-- instance-per-customer hosted offering without a schema migration.
--
-- IDs: every `id` column is an app-generated UUIDv7 stored in the native
-- 16-byte uuid type. The API renders ids as TypeIDs — `<prefix>_<uuidv7 as
-- 26-char Crockford base32>`, e.g. notif_01h455vb4pex5vsknk084sn02q — with a
-- constant per-table prefix (env_, key_, sub_, notif_, bcast_, job_). The
-- prefix is never stored: it is static per table, so storing it would be
-- redundant; uuid (not text) storage keeps the hot indexes fixed-width and
-- collation-free. A pair of SQL helpers (typeid_format/typeid_parse, shipped
-- with migrations) converts at the psql prompt so API-shaped ids from bug
-- reports are greppable directly.
--
-- ---------------------------------------------------------------------------
-- SHARD-READINESS INVARIANTS (documented, tested — like every guarantee here)
-- ---------------------------------------------------------------------------
-- The shipped deployment is one Postgres scaled vertically (the plan's
-- "scale-up over shard-out"). But this schema is kept distributable-by-
-- environment_id at all times, so moving to Citus-style sharding is an ops
-- decision, never a schema rewrite. The invariants that keep that true:
--
--   1. environment_id is in EVERY PK, UNIQUE constraint, and FK — i.e. it is
--      a valid distribution column for every distributed-Postgres engine.
--   2. No sequences anywhere. All ids are app-generated UUIDv7 — nothing
--      needs cluster-wide coordination to mint an id.
--   3. Every hot-path statement (merged list, counts, mark-read, watermark
--      moves, counter bumps, job claims, the whole create transaction) filters
--      on a single environment_id — single-shard routable. Cross-environment
--      queries exist only in admin/maintenance paths.
--   4. environments, api_keys, and broadcasts stay reference-table sized
--      (config rows / one row per announcement) — replicable to every node.
--   5. The create transaction (notification rows + counters + idempotency +
--      outbox job) touches one environment only, so it never becomes a
--      multi-shard 2PC under environment sharding.
--
-- CI enforces 1–2 with a migration lint: any new table whose PK/UNIQUE lacks
-- environment_id, or any column with a serial/sequence default, fails the
-- build (environments itself is the allowlisted root).
--
-- Known limit, on purpose: the scaling unit is the environment (consumer
-- app). Spreading ONE white-hot environment across shards would require
-- subscriber-level sharding, which breaks invariant 5 and counter
-- co-location — if that day comes, it is a redesign, not a config change.
--
-- ---------------------------------------------------------------------------
-- THE TWO-SOURCE INBOX (read this block before touching anything below)
-- ---------------------------------------------------------------------------
-- Direct notifications fan out on WRITE (one row per recipient).
-- Broadcasts fan out on READ (one row per announcement, ever).
-- The subscriber-facing inbox is the merge of both sources.
--
-- Visibility rule for broadcasts:
--     a subscriber sees a broadcast iff broadcasts.created_at >= subscribers.created_at
-- i.e. you never see announcements from before you existed. subscribers.created_at
-- is backdatable via the management upsert so customers importing existing user
-- bases can decide which historical broadcasts those users should see.
--
-- Read state is a per-subscriber WATERMARK plus per-item exceptions:
--     direct item is read    iff  read_at IS NOT NULL OR visible_at <= read_watermark
--     broadcast item is read iff  broadcast_reads row exists OR created_at <= read_watermark
-- "Mark all read" only moves the watermark (one-row UPDATE) — never a bulk
-- UPDATE over notifications. This is the MVCC-bloat avoidance on the hottest
-- write path. The same single watermark governs BOTH sources: anything whose
-- ordering timestamp is <= the watermark is read, regardless of source.
--
-- Seen state (badge semantics) is watermark-ONLY: there is no per-item seen.
-- Opening the inbox moves seen_watermark; an item is "unseen" iff its ordering
-- timestamp > seen_watermark. No exceptions table needed for seen.
--
-- Ordering spine: visible_at = COALESCE(deliver_at, created_at).
-- Scheduled notifications (deliver_at in the future) are inserted at create
-- time but excluded from all subscriber queries until visible_at <= now().
-- Pagination keysets, watermark comparisons, and partitioning ALL use
-- visible_at, never created_at, because:
--   1. born-read trap: a notification created at T0 but scheduled for T5 would
--      be instantly "read" by any watermark moved between T0 and T5 if the
--      watermark compared created_at;
--   2. retention alignment: dropping the 2026-01 partition must drop things
--      that *appeared* in January, not things scheduled in January for June.
-- For broadcasts (no deliver_at in v1) created_at IS the ordering timestamp.
--
-- Ordering timestamps are DB-clock-sourced, ALWAYS: created_at and visible_at
-- are computed by Postgres (now()) inside the INSERT — never by application
-- replicas. N stateless binaries means N clocks; app-clock skew would insert
-- items "in the past" — below a watermark (born read) or below an in-flight
-- pagination cursor (skipped forever). The CHECK on notifications makes a
-- mixed-source mistake fail loudly instead of drifting.
--
-- Canonical merged list query (keyset-paginated, newest first; $cursor is the
-- (visible_at, id) tuple of the last item of the previous page, or +infinity):
--
--   SELECT * FROM (
--     SELECT 'notification' AS source, n.id, n.category, n.payload,
--            n.visible_at AS occurred_at,
--            (n.read_at IS NOT NULL OR n.visible_at <= c.read_watermark) AS read
--       FROM notifications n
--      WHERE n.environment_id = $env AND n.subscriber_id = $sub
--        AND n.visible_at <= now()                       -- hide scheduled
--        AND (n.visible_at, n.id) < ($cursor_ts, $cursor_id)
--        AND NOT EXISTS (SELECT 1 FROM preferences p     -- read-time mute
--              WHERE p.environment_id = n.environment_id
--                AND p.subscriber_id  = n.subscriber_id
--                AND p.category = n.category AND p.channel = 'in_app'
--                AND p.enabled = false)
--      ORDER BY n.visible_at DESC, n.id DESC LIMIT $page_size
--   UNION ALL
--     SELECT 'broadcast', b.id, b.category, b.payload, b.created_at,
--            (br.broadcast_id IS NOT NULL OR b.created_at <= c.read_watermark)
--       FROM broadcasts b
--       LEFT JOIN broadcast_reads br
--              ON br.environment_id = b.environment_id
--             AND br.subscriber_id  = $sub
--             AND br.broadcast_id   = b.id
--      WHERE b.environment_id = $env
--        AND b.created_at >= $subscriber_created_at      -- visibility rule
--        AND (b.created_at, b.id) < ($cursor_ts, $cursor_id)
--        AND NOT EXISTS (... same preferences predicate on b.category ...)
--      ORDER BY b.created_at DESC, b.id DESC LIMIT $page_size
--   ) merged
--   ORDER BY occurred_at DESC, id DESC
--   LIMIT $page_size;
--
-- Each arm is independently keyset-limited (both can satisfy the page in the
-- worst case), then merged. Both arms are index-only-ish range scans:
-- notifications_inbox_idx and broadcasts_window_idx below exist precisely for
-- the two ORDER BY ... LIMIT shapes. The id tiebreaker makes the keyset total.
--
-- Canonical unread count (the hottest read — O(1)-ish, never O(rows)):
--
--   unread = subscriber_counters.unread_direct_count        -- maintained
--          + (SELECT count(*) FROM broadcasts b              -- tiny range scan:
--               WHERE b.environment_id = $env                -- one row per
--                 AND b.created_at >= $subscriber_created_at -- announcement,
--                 AND b.created_at >  c.read_watermark)      -- not per user
--          - (SELECT count(*) FROM broadcast_reads br        -- index-only via
--               WHERE br.environment_id = $env               -- denormalized
--                 AND br.subscriber_id  = $sub               -- broadcast_created_at
--                 AND br.broadcast_created_at > c.read_watermark)
--
-- The broadcast terms are real counts but over the *broadcasts* table (rows =
-- number of announcements, dozens not millions) and the subscriber's own
-- exception rows above the watermark (bounded, GC'd on watermark moves).
-- unseen count is identical with seen_watermark and NO exceptions term.
-- Category mutes are intentionally ignored in counters (documented): exact
-- mute-aware counting would make every preference flip a counter rebuild.
-- Instead, a preference change enqueues a 'counter_rebuild' job that recounts
-- that one subscriber — eventual exactness, cheap because it is per-subscriber.
--
-- Counter maintenance invariants (all inside the owning transaction):
--   * direct insert (immediately visible): counters bumped in the SAME txn
--     as the notifications insert, as a CONDITIONAL increment reading the
--     watermark inside the UPDATE itself:
--       unread_direct_count += (visible_at > read_watermark)::int
--     (unseen_direct_count likewise against seen_watermark). This is the
--     symmetric guard to the decrement rule below: without it, a
--     mark-all-read committing between this txn's now() and its commit
--     leaves the item born-read but counted — permanent +1 drift. Both
--     paths write the counters row, so the row lock serializes them; the
--     condition makes the serialization order irrelevant.
--   * scheduled insert: counters NOT bumped at create. The deliver job bumps
--     them in the SAME txn that deletes the job row — job deletion is the
--     exactly-once keying for the side effect (job gone = effect applied).
--   * individual mark-read: decrement unread_direct_count ONLY IF the row had
--     read_at IS NULL AND visible_at > read_watermark (else it was already
--     counted as read — double-decrement otherwise).
--   * mark-all-read: read_watermark = now(), unread_direct_count = 0, and
--     GC: DELETE FROM broadcast_reads WHERE broadcast_created_at <= new
--     watermark (exception rows below the watermark are redundant).
--   * mark-all-seen: seen_watermark = now(), unseen_direct_count = 0.
--   * EVERY read-state mutation — individual read_at set, broadcast_reads
--     insert, either watermark move — also touches
--     subscriber_counters.updated_at in the same txn. That column is the
--     change-detection input for the list ETag (see openapi.yaml); a
--     mutation that skips the bump makes conditional refetches serve stale
--     304s. (This is why mark-broadcast-read writes the counters row even
--     though no maintained counter changes.)
-- Redis caches the computed totals; Postgres is authoritative.
-- =============================================================================

BEGIN;

-- =============================================================================
-- environments — the isolation unit. Not hardcoded dev/prod: 'dashboard-prod',
-- 'mobile-prod', 'dashboard-dev' all coexist on one instance.
-- =============================================================================
CREATE TABLE environments (
    id          uuid        NOT NULL,           -- UUIDv7; TypeID prefix env_
    slug        text        NOT NULL,           -- URL-safe handle; the widget
                                                -- sends this to scope the
                                                -- subscriber plane
    name        text        NOT NULL,

    -- Widget auth: HMAC-SHA256(secret, external subscriber_id), computed by the
    -- customer backend. The secret is DEDICATED — deliberately not a management
    -- API key — so rotating/revoking API keys never invalidates live <Inbox />
    -- sessions. Two slots give a zero-downtime rotation overlap: verify against
    -- current then previous; the previous slot is cleared when rotation ends.
    subscriber_hmac_secret          text        NOT NULL,
    subscriber_hmac_secret_previous text,
    subscriber_hmac_rotated_at      timestamptz,

    -- Mandatory in production environments; optional in dev so the quickstart
    -- works in 30 seconds without a backend.
    require_subscriber_hash boolean NOT NULL DEFAULT true,

    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),

    PRIMARY KEY (id),
    UNIQUE (slug)
);

-- =============================================================================
-- api_keys — management-plane bearer credentials. Stored as a hash; the
-- plaintext is shown once at creation.
-- =============================================================================
CREATE TABLE api_keys (
    environment_id uuid        NOT NULL REFERENCES environments (id),
    id             uuid        NOT NULL,        -- UUIDv7; TypeID prefix key_
    name           text        NOT NULL,
    key_hash       bytea       NOT NULL,        -- sha256(full key string)
    key_prefix     text        NOT NULL,        -- e.g. 'drnt_live_ab12' for display
    created_at     timestamptz NOT NULL DEFAULT now(),
    revoked_at     timestamptz,                 -- soft revoke; rows kept for audit
    last_used_at   timestamptz,                 -- coarse (updated at most ~1/min)

    PRIMARY KEY (environment_id, id),
    UNIQUE (environment_id, key_hash)
);

-- Bearer auth resolves the key BEFORE the environment is known, so lookup is
-- by hash alone. Keys embed 256 bits of randomness — cross-environment hash
-- collisions are not a real-world event; per-env uniqueness above satisfies
-- the isolation rule while this plain index serves the hot auth path.
CREATE INDEX api_keys_key_hash_idx ON api_keys (key_hash) WHERE revoked_at IS NULL;

-- =============================================================================
-- subscribers — end users of the customer's product. Lazily upserted on first
-- notify AND on first widget connect; also explicitly upsertable via the
-- management plane (which may backdate created_at — see visibility rule).
-- Internal uuid PK (not the customer's string id) keeps FK rows in the
-- partitioned notifications table small and uniform.
-- One-subscriber-many-endpoints from day 1: the future push_subscriptions
-- table hangs off (environment_id, id); nothing here is 1:1 with a device.
-- =============================================================================
CREATE TABLE subscribers (
    environment_id uuid        NOT NULL REFERENCES environments (id),
    id             uuid        NOT NULL,        -- internal UUIDv7; TypeID prefix sub_
    subscriber_id  text        NOT NULL,        -- customer-provided ('usr_42')

    -- Drives broadcast visibility: a subscriber sees only broadcasts with
    -- broadcasts.created_at >= this value. Backdatable on import.
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now(),

    PRIMARY KEY (environment_id, id),
    UNIQUE (environment_id, subscriber_id)
);

-- =============================================================================
-- notifications — the only first-class delivery object. One row PER RECIPIENT
-- (direct fan-out on write). Monthly RANGE partitions on visible_at.
--
-- Why visible_at and not created_at as partition key: see header block
-- ("ordering spine"). visible_at is a plain column (Postgres forbids generated
-- columns as partition keys); the CHECK below pins it to the only legal value.
-- The API caps deliver_at at 13 months out, so the partition-maintenance job
-- (boot + daily) pre-creates partitions covering [now-retention, now+13mo].
-- No DEFAULT partition on purpose: rows landing in a DEFAULT partition block
-- later creation of their proper partition; a missing partition is a loud
-- insert error caught in CI, which we prefer to silent unprunable growth.
--
-- The global UNIQUE (environment_id, idempotency_key) the plan calls for
-- CANNOT live here: Postgres requires partition keys in every unique index of
-- a partitioned table. Idempotency is enforced in idempotency_keys below.
-- =============================================================================
CREATE TABLE notifications (
    environment_id uuid        NOT NULL,
    id             uuid        NOT NULL,        -- UUIDv7 (time-ordered for index
                                                -- locality; keyset tiebreaker);
                                                -- TypeID prefix notif_
    subscriber_id  uuid        NOT NULL,        -- internal subscribers.id
    category       text        NOT NULL,        -- customer-defined ('payment.failed');
                                                -- no registry table — categories are
                                                -- an open string namespace
    payload        jsonb       NOT NULL DEFAULT '{}'::jsonb,  -- typed payload,
                                                -- rendered client-side; no
                                                -- server-side templates
    created_at     timestamptz NOT NULL DEFAULT now(),
    deliver_at     timestamptz,                 -- NULL = immediate
    visible_at     timestamptz NOT NULL,        -- ordering spine; see header
    read_at        timestamptz,                 -- per-item read exception; an item
                                                -- is read iff read_at IS NOT NULL
                                                -- OR visible_at <= read_watermark

    PRIMARY KEY (environment_id, id, visible_at),
    FOREIGN KEY (environment_id, subscriber_id)
        REFERENCES subscribers (environment_id, id),
    CONSTRAINT notifications_visible_at_is_coalesce
        CHECK (visible_at = COALESCE(deliver_at, created_at))
) PARTITION BY RANGE (visible_at);

-- The merged-list arm and per-subscriber scans. DESC matches the only query
-- shape (newest first, keyset). Partition pruning applies when the cursor
-- bounds visible_at.
CREATE INDEX notifications_inbox_idx
    ON notifications (environment_id, subscriber_id, visible_at DESC, id DESC);

-- Example partitions; the server creates these automatically (boot + daily
-- maintenance job, under the same advisory lock as migrations). Retention =
-- DETACH + DROP of expired partitions, configured in months.
CREATE TABLE notifications_2026_06 PARTITION OF notifications
    FOR VALUES FROM ('2026-06-01+00') TO ('2026-07-01+00');
CREATE TABLE notifications_2026_07 PARTITION OF notifications
    FOR VALUES FROM ('2026-07-01+00') TO ('2026-08-01+00');

-- =============================================================================
-- broadcasts — one row per announcement, targeting the whole environment.
-- NEVER materialized per subscriber: "announce to all" is one insert
-- regardless of subscriber count (fan-out on read). Topic targeting is a
-- deliberate v1 omission; adding it later is additive (topics +
-- subscriber_topics tables, nullable topic_id here, one OR in the merge).
-- No deliver_at in v1 (additive later). Not partitioned: row count is the
-- number of announcements ever made — small by construction.
-- =============================================================================
CREATE TABLE broadcasts (
    environment_id uuid        NOT NULL REFERENCES environments (id),
    id             uuid        NOT NULL,        -- UUIDv7; TypeID prefix bcast_
    category       text        NOT NULL,
    payload        jsonb       NOT NULL DEFAULT '{}'::jsonb,
    created_at     timestamptz NOT NULL DEFAULT now(),  -- the ordering timestamp
                                                        -- AND the visibility-rule
                                                        -- comparand

    PRIMARY KEY (environment_id, id)
);

-- Serves both the merged-list broadcast arm and the unread-count window scan
-- (created_at >= subscriber.created_at AND created_at > watermark).
CREATE INDEX broadcasts_window_idx
    ON broadcasts (environment_id, created_at DESC, id DESC);

-- =============================================================================
-- broadcast_reads — EXCEPTION table for broadcasts read individually while
-- still above the read watermark. Deliberately not a per-subscriber broadcast
-- materialization: rows exist only for (subscriber, broadcast) pairs the
-- subscriber explicitly read, and rows below the watermark are GC'd on every
-- mark-all-read (they become redundant: the watermark already covers them).
-- Steady-state size per subscriber ≈ items read since last mark-all-read.
-- =============================================================================
CREATE TABLE broadcast_reads (
    environment_id       uuid        NOT NULL,
    subscriber_id        uuid        NOT NULL,
    broadcast_id         uuid        NOT NULL,
    -- Denormalized copy of broadcasts.created_at. Makes the unread-count
    -- exceptions term and the GC delete index-only — no join back to
    -- broadcasts on the hottest read. Immutable at the source, so the
    -- denormalization cannot drift.
    broadcast_created_at timestamptz NOT NULL,
    read_at              timestamptz NOT NULL DEFAULT now(),

    PRIMARY KEY (environment_id, subscriber_id, broadcast_id),
    FOREIGN KEY (environment_id, broadcast_id)
        REFERENCES broadcasts (environment_id, id) ON DELETE CASCADE,
    FOREIGN KEY (environment_id, subscriber_id)
        REFERENCES subscribers (environment_id, id) ON DELETE CASCADE
);

-- The unread-count exceptions term and watermark GC scan.
CREATE INDEX broadcast_reads_window_idx
    ON broadcast_reads (environment_id, subscriber_id, broadcast_created_at);

-- =============================================================================
-- subscriber_counters — one hot row per subscriber: maintained direct counters
-- + both watermarks. Created on first notification/connect alongside the
-- subscriber. count(*) is O(unread) and unread is the hottest read — these
-- counters make it O(1) + a tiny broadcasts range count (see header).
-- Watermarks live HERE (not on subscribers) so the hot write path touches
-- exactly one row, and identity rows stay cold.
-- fillfactor 70: this row is updated on every insert/read/seen — leave room
-- for HOT updates so the PK index doesn't churn.
-- =============================================================================
CREATE TABLE subscriber_counters (
    environment_id      uuid        NOT NULL,
    subscriber_id       uuid        NOT NULL,

    unread_direct_count integer     NOT NULL DEFAULT 0,
    unseen_direct_count integer     NOT NULL DEFAULT 0,

    -- 'epoch' (not NULL, not now()) so a brand-new subscriber has everything
    -- visible-to-them unread/unseen, and watermark predicates never need a
    -- NULL branch.
    read_watermark      timestamptz NOT NULL DEFAULT 'epoch',
    seen_watermark      timestamptz NOT NULL DEFAULT 'epoch',

    updated_at          timestamptz NOT NULL DEFAULT now(),

    PRIMARY KEY (environment_id, subscriber_id),
    FOREIGN KEY (environment_id, subscriber_id)
        REFERENCES subscribers (environment_id, id) ON DELETE CASCADE
) WITH (fillfactor = 70);

-- =============================================================================
-- jobs — transactional outbox AND job queue in one table. A notification
-- insert and its job row commit in ONE transaction (no dual-write: a Redis
-- outage delays hints, never loses notifications). Workers claim with
-- FOR UPDATE SKIP LOCKED and DELETE on completion — never status-flag in
-- place. Completed work leaves no row (optional jobs_archive is a Phase 3
-- add), which is what keeps this table small enough for the aggressive
-- autovacuum below to keep up.
--
-- Job types in v1:
--   'hint'            — debounced Redis pub/sub publish (at-least-once is fine:
--                       hints are refetch triggers, not transports)
--   'deliver'         — scheduled notification coming due (run_at = deliver_at):
--                       bumps counters + enqueues hint IN THE SAME TXN that
--                       deletes this row — deletion is the exactly-once key
--   'counter_rebuild' — recount one subscriber after a preference change
--   (post-launch)     — 'push_fanout' etc. reuse progress_cursor below
--
-- Claiming is per-environment fair: the worker round-robins environments with
-- pending work (one SKIP LOCKED claim per env per sweep) so a broadcast burst
-- from 'dashboard-prod' cannot starve 'mobile-prod' real-time jobs.
--
-- Documented ceiling: low thousands of jobs/sec. This is an inbox, not Kafka.
-- =============================================================================
CREATE TABLE jobs (
    environment_id  uuid        NOT NULL REFERENCES environments (id),
    id              uuid        NOT NULL,       -- UUIDv7; TypeID prefix job_
    job_type        text        NOT NULL,
    payload         jsonb       NOT NULL DEFAULT '{}'::jsonb,
    run_at          timestamptz NOT NULL DEFAULT now(),  -- deliver_at scheduling
                                                         -- and retry backoff both
                                                         -- land here
    attempts        integer     NOT NULL DEFAULT 0,
    max_attempts    integer     NOT NULL DEFAULT 10,     -- exhausted → parked for
                                                         -- DLQ replay (Phase 3)
    last_error      text,

    -- Resumable chunked fan-outs: a large job (future broadcast-to-push over
    -- millions of endpoints) processes a chunk, advances this cursor, and
    -- COMMITs — never one giant transaction, never N tiny rows. A crashed
    -- worker's successor resumes from the cursor; chunk effects are keyed so
    -- replaying the last uncommitted chunk is safe.
    progress_cursor jsonb,

    created_at      timestamptz NOT NULL DEFAULT now(),

    PRIMARY KEY (environment_id, id)
)
WITH (
    -- MVCC hygiene for a high-churn queue: vacuum by threshold not scale
    -- factor (table should be near-empty at steady state), and low fillfactor
    -- so attempt/cursor updates stay HOT.
    fillfactor = 50,
    autovacuum_vacuum_scale_factor = 0,
    autovacuum_vacuum_threshold = 500,
    autovacuum_vacuum_cost_delay = 0
);

-- The claim scan, per environment (fair round-robin needs the env prefix):
--   SELECT ... WHERE environment_id = $env AND run_at <= now()
--   ORDER BY run_at LIMIT $n FOR UPDATE SKIP LOCKED
CREATE INDEX jobs_claim_idx ON jobs (environment_id, run_at);

-- =============================================================================
-- idempotency_keys — enforces UNIQUE (environment_id, idempotency_key) per
-- resource type. Lives OUTSIDE notifications because Postgres cannot enforce
-- a global unique constraint on a partitioned table without the partition key
-- in it (which would break idempotency across month boundaries).
--
-- Also the natural shape for batch creates: one key maps to N notification
-- rows; response_snapshot stores the full original response so a retry is
-- acknowledged-and-dropped with a byte-identical body (and never re-runs the
-- batch partially). Insert of this row, the notification rows, the counter
-- bumps, and the outbox job all commit in one transaction; a unique violation
-- here means "retry" and short-circuits to the snapshot.
--
-- Purged by age (default 30 days) via the maintenance job — comfortably
-- longer than any sane client retry horizon.
-- =============================================================================
CREATE TABLE idempotency_keys (
    environment_id    uuid        NOT NULL REFERENCES environments (id),
    scope             text        NOT NULL,     -- 'notification' | 'broadcast'
    idempotency_key   text        NOT NULL,     -- client-supplied or
                                                -- server-generated (echoed)
    response_snapshot jsonb       NOT NULL,
    created_at        timestamptz NOT NULL DEFAULT now(),

    PRIMARY KEY (environment_id, scope, idempotency_key)
);

CREATE INDEX idempotency_keys_purge_idx ON idempotency_keys (created_at);

-- =============================================================================
-- preferences — per-subscriber, per-category, per-channel mute.
-- Row ABSENCE means enabled: the table only stores explicit choices, so the
-- common case (no preference set) costs zero rows and the read-time predicate
-- is a NOT EXISTS on enabled = false.
--
-- channel is 'in_app'-only in v1 but exists NOW so push transports never need
-- a preferences migration. Deliberately NO CHECK constraint on channel values:
-- adding 'web_push' must be a code change, not DDL on a hot table. The API
-- layer owns the allowed-values list.
--
-- Evaluated at READ time for in_app (merge query filters muted categories);
-- will be evaluated at SEND time for push transports. Counters ignore mutes;
-- a preference flip enqueues 'counter_rebuild' for exactness (see header).
-- =============================================================================
CREATE TABLE preferences (
    environment_id uuid        NOT NULL,
    subscriber_id  uuid        NOT NULL,
    category       text        NOT NULL,
    channel        text        NOT NULL DEFAULT 'in_app',
    enabled        boolean     NOT NULL,
    updated_at     timestamptz NOT NULL DEFAULT now(),

    PRIMARY KEY (environment_id, subscriber_id, category, channel),
    FOREIGN KEY (environment_id, subscriber_id)
        REFERENCES subscribers (environment_id, id) ON DELETE CASCADE
);

COMMIT;
