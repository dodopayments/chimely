-- Expiring (v2) subscriber hashes.
--
-- require_subscriber_hash_expiry refuses every hash without an expiry,
-- including legacy hashes. The default keeps legacy hashes valid.
-- subscriber_hash_max_ttl_seconds caps how far in the future a v2 expiry
-- may be.

ALTER TABLE environments
    ADD COLUMN require_subscriber_hash_expiry boolean NOT NULL DEFAULT false,
    ADD COLUMN subscriber_hash_max_ttl_seconds integer NOT NULL DEFAULT 86400
        CHECK (subscriber_hash_max_ttl_seconds > 0);
