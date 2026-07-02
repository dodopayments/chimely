-- Archive state, mirroring the read-state shape exactly: a per-subscriber
-- archive watermark for bulk archive-all, plus per-item overrides that
-- outrank it in either direction.
--
-- Direct: archived_at/unarchived_at are sibling columns, never both set.
-- Broadcast: broadcast_archives is a separate override table, NOT extra
-- columns on broadcast_reads, because read and archive have independent
-- watermark GCs (mark-all-read deletes broadcast_reads rows at or below the
-- READ watermark and must not destroy archive state).
--
-- No backfill: column/row absence = the watermark decides, and the epoch
-- default archives nothing.

ALTER TABLE subscriber_counters
    ADD COLUMN archive_watermark timestamptz NOT NULL DEFAULT 'epoch';

ALTER TABLE notifications
    ADD COLUMN archived_at timestamptz,
    ADD COLUMN unarchived_at timestamptz;

CREATE TABLE broadcast_archives (
    environment_id       uuid        NOT NULL,
    subscriber_id        uuid        NOT NULL,
    broadcast_id         uuid        NOT NULL,
    -- Immutable copy of broadcasts.created_at so the archive-all GC can
    -- range-delete without joining broadcasts.
    broadcast_created_at timestamptz NOT NULL,
    -- true = archived above the watermark, false = unarchived below it.
    archived             boolean     NOT NULL,
    updated_at           timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (environment_id, subscriber_id, broadcast_id),
    FOREIGN KEY (environment_id, broadcast_id)
        REFERENCES broadcasts (environment_id, id) ON DELETE CASCADE,
    FOREIGN KEY (environment_id, subscriber_id)
        REFERENCES subscribers (environment_id, id) ON DELETE CASCADE
);

CREATE INDEX broadcast_archives_window_idx
    ON broadcast_archives (environment_id, subscriber_id, broadcast_created_at);

-- Serves the archive-all override GC. Partial: explicit exceptions only.
CREATE INDEX notifications_archive_exception_idx
    ON notifications (environment_id, subscriber_id, visible_at)
    WHERE archived_at IS NOT NULL OR unarchived_at IS NOT NULL;
