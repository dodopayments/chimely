-- Phase 4 admin multi-user auth (docs/superpowers design 2026-06-15).
--
-- Two INSTANCE-LEVEL tables: no environment_id. The admin plane is instance
-- config, like the environments root, so these tables are allowlisted in the
-- migration lint (risk W2) alongside environments and _sqlx_migrations. This
-- is still single-org: no organizations table, no per-environment user
-- scoping. Roles are instance-wide (one role per user, every environment).
--
-- This reverses the "no admin users, no roles" Phase 4 invariant FOR THE
-- ADMIN PLANE ONLY. It introduces no org concept.

-- =============================================================================
-- admin_users — operators of the embedded /admin dashboard. Email is the
-- login identity, stored already-lowercased so UNIQUE (email) enforces
-- case-insensitive identity without a functional index. role is text without
-- a CHECK (the preferences.channel precedent): the API layer owns the
-- allowed-values list, so adding a role later is a code change, not DDL.
-- =============================================================================
CREATE TABLE admin_users (
    id            uuid        NOT NULL,           -- UUIDv7; TypeID prefix adm_
    email         text        NOT NULL,           -- login identity, lowercased
    name          text        NOT NULL,
    role          text        NOT NULL,           -- 'viewer'|'operator'|'developer'|'admin'
    password_hash text        NOT NULL,           -- Argon2id PHC string
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    disabled_at   timestamptz,                    -- soft-disable; kept for audit

    PRIMARY KEY (id),
    UNIQUE (email)
);

-- =============================================================================
-- admin_sessions — server-side sessions so logout, expiry, disable-user, and
-- admin-revoke all work (a stateless token cannot be revoked). id is the
-- opaque 256-bit random token carried in the HttpOnly cookie; it is the only
-- secret and is never logged. Deleting/disabling a user invalidates their
-- sessions (ON DELETE CASCADE, plus a live disabled_at check at resolve time).
-- Idle/expired rows are GC'd by the maintenance job on expires_at.
-- =============================================================================
CREATE TABLE admin_sessions (
    id           text        NOT NULL,            -- opaque 256-bit random token (hex); the cookie value
    user_id      uuid        NOT NULL REFERENCES admin_users (id) ON DELETE CASCADE,
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    expires_at   timestamptz NOT NULL,

    PRIMARY KEY (id)
);

CREATE INDEX admin_sessions_user_idx ON admin_sessions (user_id);
-- The maintenance job deletes WHERE expires_at < now().
CREATE INDEX admin_sessions_expiry_idx ON admin_sessions (expires_at);
