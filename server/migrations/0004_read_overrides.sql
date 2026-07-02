-- Per-item unread overrides.
--
-- Read state was previously monotonic per item: read_at set once (direct),
-- exception row present (broadcast), or covered by the read watermark.
-- Mark-as-unread needs an override that survives the watermark.
--
-- Direct: unread_at is the sibling of read_at. Never both set. An item
-- at-or-below the read watermark with unread_at set is explicitly unread.
-- Above the watermark, absence of read_at already means unread, so
-- mark-unread there just clears read_at and leaves unread_at NULL.
--
-- Broadcast: broadcast_reads widens into a per-item read override. Row
-- present = the boolean decides, row absent = the watermark decides.
-- Existing rows are explicit reads, so DEFAULT true reinterprets them
-- correctly with no backfill.

ALTER TABLE notifications
    ADD COLUMN unread_at timestamptz;

ALTER TABLE broadcast_reads
    ADD COLUMN read boolean NOT NULL DEFAULT true;

-- Serves the mark-all-read override GC and the counter recount term.
-- Partial: only explicit exceptions, the same cardinality class as
-- broadcast_reads rows.
CREATE INDEX notifications_unread_exception_idx
    ON notifications (environment_id, subscriber_id, visible_at)
    WHERE unread_at IS NOT NULL;
