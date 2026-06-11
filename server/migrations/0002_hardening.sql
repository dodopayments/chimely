-- Phase 3 hardening: the dead-letter table and the per-notification status
-- log (specs/phase-3-hardening.md, deliverables 1 and 2).

-- =============================================================================
-- dead_letters — jobs that exhausted max_attempts, parked for replay. A
-- separate table on purpose: jobs are deleted on completion and the jobs
-- table stays near-empty at steady state, so parked rows must not live in
-- the hot claim path. A parked job is not a completed job; replay moves the
-- row back into jobs with attempts reset.
-- =============================================================================
CREATE TABLE dead_letters (
    environment_id  uuid        NOT NULL REFERENCES environments (id),
    id              uuid        NOT NULL,       -- original jobs.id, kept stable across park/replay
    job_type        text        NOT NULL,
    payload         jsonb       NOT NULL,
    attempts        integer     NOT NULL,
    max_attempts    integer     NOT NULL,
    last_error      text        NOT NULL,
    progress_cursor jsonb,                      -- a parked chunked job resumes from its cursor
    created_at      timestamptz NOT NULL,       -- original enqueue time
    parked_at       timestamptz NOT NULL DEFAULT now(),

    PRIMARY KEY (environment_id, id)
);

-- =============================================================================
-- notification_status_log — append-only status timeline per DIRECT
-- notification (created -> delivered_hint -> seen -> read). Broadcasts are
-- never materialized per subscriber and therefore have no per-recipient
-- timeline. Rows are only ever INSERTed; no UPDATE ever touches a row.
--
-- Monthly RANGE partitions on occurred_at, maintained by the same
-- partition-maintenance job as notifications, with the same retention
-- horizon and deliberately no DEFAULT partition. A partitioned table cannot
-- carry a global UNIQUE (environment_id, notification_id, status), so
-- exactly-once appends are enforced by the writers instead: every append
-- commits in the same transaction as its idempotency key (the notification
-- insert, the read_at flip, the hint job deletion or the timeline job
-- cursor advance) and is guarded by NOT EXISTS under the subscriber's
-- counters-row lock.
--
-- status is text without a CHECK on purpose (the preferences.channel
-- precedent): the API layer owns the allowed-values list, so push delivery
-- receipts later add values with no migration.
-- =============================================================================
CREATE TABLE notification_status_log (
    environment_id  uuid        NOT NULL,
    notification_id uuid        NOT NULL,
    status          text        NOT NULL,
    occurred_at     timestamptz NOT NULL,

    PRIMARY KEY (environment_id, notification_id, status, occurred_at)
) PARTITION BY RANGE (occurred_at);

-- No FK to notifications: FK rows would block partition retention drops on
-- the parent table. Orphan timeline rows age out on the same horizon.

-- The hint worker coalesces duplicate pending hints on every hint claim.
-- Without this partial index that lookup walks the environment's whole
-- backlog per claim, which under a deep backlog turns the drain quadratic.
CREATE INDEX jobs_hint_coalesce_idx ON jobs (environment_id)
    WHERE job_type = 'hint';
