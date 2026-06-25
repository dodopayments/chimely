-- Chimely v1 core schema. Translated 1:1 from specs/schema.sql (the frozen
-- contract — see its header for the two-source inbox, watermark, counter, and
-- shard-readiness invariants). Differences from the spec file, both sanctioned
-- by it:
--   * the example monthly partitions are not created here — the partition
--     maintenance job (boot + daily, advisory-locked) owns partition DDL;
--   * typeid_format/typeid_parse ship here (the spec header promises them
--     "with migrations").
-- sqlx's migrator runs this under a Postgres advisory lock, so N replicas
-- racing on boot apply it exactly once.

-- =============================================================================
-- TypeID helpers: <prefix>_<uuid as 26-char Crockford base32>. The 128-bit
-- uuid is left-padded with 2 zero bits to 130 = 26 * 5; first char is [0-7].
-- =============================================================================
CREATE FUNCTION typeid_format(id uuid, prefix text) RETURNS text
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE
    alphabet constant text  := '0123456789abcdefghjkmnpqrstvwxyz';
    bytes             bytea := uuid_send(id);
    acc               int   := 0;
    bits              int   := 2;  -- two zero pad bits
    suffix            text  := '';
BEGIN
    FOR i IN 0..15 LOOP
        acc  := (acc << 8) | get_byte(bytes, i);
        bits := bits + 8;
        WHILE bits >= 5 LOOP
            bits   := bits - 5;
            suffix := suffix || substr(alphabet, ((acc >> bits) & 31) + 1, 1);
            acc    := acc & ((1 << bits) - 1);
        END LOOP;
    END LOOP;
    RETURN CASE WHEN prefix = '' THEN suffix ELSE prefix || '_' || suffix END;
END;
$$;

CREATE FUNCTION typeid_parse(typeid text) RETURNS uuid
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE
    alphabet constant text := '0123456789abcdefghjkmnpqrstvwxyz';
    suffix            text;
    acc               int  := 0;
    bits              int;
    v                 int;
    hex               text := '';
BEGIN
    IF length(typeid) < 26 THEN
        RAISE EXCEPTION 'invalid typeid: %', typeid;
    END IF;
    IF length(typeid) > 26 AND substr(typeid, length(typeid) - 26, 1) <> '_' THEN
        RAISE EXCEPTION 'invalid typeid (malformed prefix): %', typeid;
    END IF;
    suffix := right(typeid, 26);

    -- First char carries only 3 significant bits (the 2 pad bits are zero).
    v := strpos(alphabet, substr(suffix, 1, 1)) - 1;
    IF v < 0 OR v > 7 THEN
        RAISE EXCEPTION 'invalid typeid suffix: %', suffix;
    END IF;
    acc  := v;
    bits := 3;
    FOR i IN 2..26 LOOP
        v := strpos(alphabet, substr(suffix, i, 1)) - 1;
        IF v < 0 THEN
            RAISE EXCEPTION 'invalid typeid suffix: %', suffix;
        END IF;
        acc  := (acc << 5) | v;
        bits := bits + 5;
        IF bits >= 8 THEN
            bits := bits - 8;
            hex  := hex || lpad(to_hex((acc >> bits) & 255), 2, '0');
            acc  := acc & ((1 << bits) - 1);
        END IF;
    END LOOP;
    RETURN hex::uuid;
END;
$$;

-- =============================================================================
-- environments — the isolation unit. Not hardcoded dev/prod: 'dashboard-prod',
-- 'mobile-prod', 'dashboard-dev' all coexist on one instance.
-- =============================================================================
CREATE TABLE environments (
    id          uuid        NOT NULL,           -- UUIDv7; TypeID prefix env_
    slug        text        NOT NULL,
    name        text        NOT NULL,

    -- Widget auth: HMAC-SHA256(secret, external subscriber_id). Dedicated
    -- secret (not a management API key) with two slots for zero-downtime
    -- rotation: verify against current then previous.
    subscriber_hmac_secret          text        NOT NULL,
    subscriber_hmac_secret_previous text,
    subscriber_hmac_rotated_at      timestamptz,

    require_subscriber_hash boolean NOT NULL DEFAULT true,

    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),

    PRIMARY KEY (id),
    UNIQUE (slug)
);

-- =============================================================================
-- api_keys — management-plane bearer credentials, stored hashed.
-- =============================================================================
CREATE TABLE api_keys (
    environment_id uuid        NOT NULL REFERENCES environments (id),
    id             uuid        NOT NULL,        -- UUIDv7; TypeID prefix key_
    name           text        NOT NULL,
    key_hash       bytea       NOT NULL,        -- sha256(full key string)
    key_prefix     text        NOT NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    revoked_at     timestamptz,
    last_used_at   timestamptz,                 -- coarse (updated at most ~1/min)

    PRIMARY KEY (environment_id, id),
    UNIQUE (environment_id, key_hash)
);

-- Bearer auth resolves the key BEFORE the environment is known.
CREATE INDEX api_keys_key_hash_idx ON api_keys (key_hash) WHERE revoked_at IS NULL;

-- =============================================================================
-- subscribers — end users of the customer's product; lazily upserted.
-- created_at drives broadcast visibility and is backdatable on import.
-- =============================================================================
CREATE TABLE subscribers (
    environment_id uuid        NOT NULL REFERENCES environments (id),
    id             uuid        NOT NULL,        -- internal UUIDv7; TypeID prefix sub_
    subscriber_id  text        NOT NULL,        -- customer-provided ('usr_42')

    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now(),

    PRIMARY KEY (environment_id, id),
    UNIQUE (environment_id, subscriber_id)
);

-- =============================================================================
-- notifications — one row PER RECIPIENT (direct fan-out on write). Monthly
-- RANGE partitions on visible_at (the ordering spine). No DEFAULT partition
-- on purpose: a missing partition is a loud insert error.
-- =============================================================================
CREATE TABLE notifications (
    environment_id uuid        NOT NULL,
    id             uuid        NOT NULL,        -- UUIDv7; TypeID prefix notif_
    subscriber_id  uuid        NOT NULL,        -- internal subscribers.id
    category       text        NOT NULL,
    payload        jsonb       NOT NULL DEFAULT '{}'::jsonb,
    created_at     timestamptz NOT NULL DEFAULT now(),
    deliver_at     timestamptz,                 -- NULL = immediate
    visible_at     timestamptz NOT NULL,        -- ordering spine
    read_at        timestamptz,                 -- per-item read exception

    PRIMARY KEY (environment_id, id, visible_at),
    FOREIGN KEY (environment_id, subscriber_id)
        REFERENCES subscribers (environment_id, id),
    CONSTRAINT notifications_visible_at_is_coalesce
        CHECK (visible_at = COALESCE(deliver_at, created_at))
) PARTITION BY RANGE (visible_at);

CREATE INDEX notifications_inbox_idx
    ON notifications (environment_id, subscriber_id, visible_at DESC, id DESC);

-- =============================================================================
-- broadcasts — one row per announcement (fan-out on read), never materialized
-- per subscriber. Not partitioned: small by construction.
-- =============================================================================
CREATE TABLE broadcasts (
    environment_id uuid        NOT NULL REFERENCES environments (id),
    id             uuid        NOT NULL,        -- UUIDv7; TypeID prefix bcast_
    category       text        NOT NULL,
    payload        jsonb       NOT NULL DEFAULT '{}'::jsonb,
    created_at     timestamptz NOT NULL DEFAULT now(),

    PRIMARY KEY (environment_id, id)
);

CREATE INDEX broadcasts_window_idx
    ON broadcasts (environment_id, created_at DESC, id DESC);

-- =============================================================================
-- broadcast_reads — EXCEPTION rows for broadcasts read individually above the
-- watermark; GC'd on every mark-all-read.
-- =============================================================================
CREATE TABLE broadcast_reads (
    environment_id       uuid        NOT NULL,
    subscriber_id        uuid        NOT NULL,
    broadcast_id         uuid        NOT NULL,
    broadcast_created_at timestamptz NOT NULL,  -- denormalized; immutable source
    read_at              timestamptz NOT NULL DEFAULT now(),

    PRIMARY KEY (environment_id, subscriber_id, broadcast_id),
    FOREIGN KEY (environment_id, broadcast_id)
        REFERENCES broadcasts (environment_id, id) ON DELETE CASCADE,
    FOREIGN KEY (environment_id, subscriber_id)
        REFERENCES subscribers (environment_id, id) ON DELETE CASCADE
);

CREATE INDEX broadcast_reads_window_idx
    ON broadcast_reads (environment_id, subscriber_id, broadcast_created_at);

-- =============================================================================
-- subscriber_counters — one hot row per subscriber: maintained direct counters
-- + both watermarks. fillfactor 70 for HOT updates.
-- =============================================================================
CREATE TABLE subscriber_counters (
    environment_id      uuid        NOT NULL,
    subscriber_id       uuid        NOT NULL,

    unread_direct_count integer     NOT NULL DEFAULT 0,
    unseen_direct_count integer     NOT NULL DEFAULT 0,

    read_watermark      timestamptz NOT NULL DEFAULT 'epoch',
    seen_watermark      timestamptz NOT NULL DEFAULT 'epoch',

    updated_at          timestamptz NOT NULL DEFAULT now(),

    PRIMARY KEY (environment_id, subscriber_id),
    FOREIGN KEY (environment_id, subscriber_id)
        REFERENCES subscribers (environment_id, id) ON DELETE CASCADE
) WITH (fillfactor = 70);

-- =============================================================================
-- jobs — transactional outbox AND job queue. Claimed FOR UPDATE SKIP LOCKED,
-- DELETEd on completion. v1 types: 'hint', 'deliver', 'counter_rebuild'.
-- =============================================================================
CREATE TABLE jobs (
    environment_id  uuid        NOT NULL REFERENCES environments (id),
    id              uuid        NOT NULL,       -- UUIDv7; TypeID prefix job_
    job_type        text        NOT NULL,
    payload         jsonb       NOT NULL DEFAULT '{}'::jsonb,
    run_at          timestamptz NOT NULL DEFAULT now(),
    attempts        integer     NOT NULL DEFAULT 0,
    max_attempts    integer     NOT NULL DEFAULT 10,
    last_error      text,
    progress_cursor jsonb,                      -- resumable chunked fan-outs
    created_at      timestamptz NOT NULL DEFAULT now(),

    PRIMARY KEY (environment_id, id)
)
WITH (
    fillfactor = 50,
    autovacuum_vacuum_scale_factor = 0,
    autovacuum_vacuum_threshold = 500,
    autovacuum_vacuum_cost_delay = 0
);

CREATE INDEX jobs_claim_idx ON jobs (environment_id, run_at);

-- =============================================================================
-- idempotency_keys — UNIQUE (environment_id, scope, idempotency_key); stores
-- the full original response for byte-identical replay. Purged by age.
-- =============================================================================
CREATE TABLE idempotency_keys (
    environment_id    uuid        NOT NULL REFERENCES environments (id),
    scope             text        NOT NULL,     -- 'notification' | 'broadcast'
    idempotency_key   text        NOT NULL,
    response_snapshot jsonb       NOT NULL,
    created_at        timestamptz NOT NULL DEFAULT now(),

    PRIMARY KEY (environment_id, scope, idempotency_key)
);

CREATE INDEX idempotency_keys_purge_idx ON idempotency_keys (created_at);

-- =============================================================================
-- preferences — per-subscriber, per-category, per-channel mute. Row ABSENCE
-- means enabled. channel deliberately has NO CHECK constraint (the API layer
-- owns the allowed-values list).
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
