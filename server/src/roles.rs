//! Admin-plane roles and capabilities (docs/superpowers design 2026-06-15).
//!
//! Capabilities are the unit of enforcement; the four roles are fixed presets.
//! `admin` holds every capability. `operator` and `developer` are parallel
//! branches above `viewer`, neither a superset of the other. Roles are
//! instance-wide (one role per user, every environment) — still single-org,
//! no per-environment scoping.
//!
//! `role` is stored as text without a DB CHECK (the preferences.channel
//! precedent): this module is the single source of truth for the allowed
//! values, so adding a role is a code change, not a migration.

use std::fmt;

/// A unit of authorization. Every gated admin endpoint requires one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    /// Inbox/notifications/timelines, subscriber lookup, environment list
    /// (no secret), DLQ list, dashboard.
    Read,
    DlqReplay,
    BroadcastCompose,
    /// API-key prefix + metadata.
    ApikeyRead,
    /// Create / revoke API keys.
    ApikeyManage,
    /// An environment's `subscriber_hmac_secret` (needed to wire up the widget).
    EnvReadSecret,
    EnvCreate,
    HmacRotate,
    UserManage,
}

impl Capability {
    /// The wire string the `/admin/api/me` response exposes so the SPA can
    /// gate UI. Mirrors the capability column headers in the design table.
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Read => "read",
            Capability::DlqReplay => "dlq:replay",
            Capability::BroadcastCompose => "broadcast:compose",
            Capability::ApikeyRead => "apikey:read",
            Capability::ApikeyManage => "apikey:manage",
            Capability::EnvReadSecret => "env:read_secret",
            Capability::EnvCreate => "env:create",
            Capability::HmacRotate => "hmac:rotate",
            Capability::UserManage => "user:manage",
        }
    }
}

/// A fixed role preset. One per user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Viewer,
    Operator,
    Developer,
    Admin,
}

/// The allowed `role` values, in the order the design table lists them.
pub const ALL_ROLES: [Role; 4] = [Role::Viewer, Role::Operator, Role::Developer, Role::Admin];

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Operator => "operator",
            Role::Developer => "developer",
            Role::Admin => "admin",
        }
    }

    /// Parse a stored/wire role string. Unknown values are rejected (the API
    /// owns the allowed-values list).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "viewer" => Some(Role::Viewer),
            "operator" => Some(Role::Operator),
            "developer" => Some(Role::Developer),
            "admin" => Some(Role::Admin),
            _ => None,
        }
    }

    /// The capabilities this role holds, in a stable order for the `/me`
    /// response. `developer` carries `env:read_secret` because computing
    /// subscriber hashes in a backend needs the secret, so it rides with
    /// `apikey:read`.
    pub fn capabilities(self) -> &'static [Capability] {
        use Capability::*;
        match self {
            Role::Viewer => &[Read],
            Role::Operator => &[Read, DlqReplay, BroadcastCompose],
            Role::Developer => &[Read, ApikeyRead, ApikeyManage, EnvReadSecret],
            Role::Admin => &[
                Read,
                DlqReplay,
                BroadcastCompose,
                ApikeyRead,
                ApikeyManage,
                EnvReadSecret,
                EnvCreate,
                HmacRotate,
                UserManage,
            ],
        }
    }

    pub fn has(self, cap: Capability) -> bool {
        self.capabilities().contains(&cap)
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_strings_round_trip() {
        for role in ALL_ROLES {
            assert_eq!(Role::parse(role.as_str()), Some(role));
        }
        assert_eq!(Role::parse("root"), None);
        assert_eq!(Role::parse(""), None);
    }

    #[test]
    fn admin_holds_every_capability() {
        for cap in [
            Capability::Read,
            Capability::DlqReplay,
            Capability::BroadcastCompose,
            Capability::ApikeyRead,
            Capability::ApikeyManage,
            Capability::EnvReadSecret,
            Capability::EnvCreate,
            Capability::HmacRotate,
            Capability::UserManage,
        ] {
            assert!(Role::Admin.has(cap), "admin missing {}", cap.as_str());
        }
    }

    #[test]
    fn presets_match_the_design_matrix() {
        // viewer: read only.
        assert!(Role::Viewer.has(Capability::Read));
        assert!(!Role::Viewer.has(Capability::DlqReplay));
        assert!(!Role::Viewer.has(Capability::EnvReadSecret));

        // operator and developer are parallel branches above viewer, neither
        // a superset of the other.
        assert!(Role::Operator.has(Capability::DlqReplay));
        assert!(Role::Operator.has(Capability::BroadcastCompose));
        assert!(!Role::Operator.has(Capability::ApikeyRead));
        assert!(!Role::Operator.has(Capability::EnvReadSecret));

        assert!(Role::Developer.has(Capability::ApikeyRead));
        assert!(Role::Developer.has(Capability::ApikeyManage));
        assert!(Role::Developer.has(Capability::EnvReadSecret));
        assert!(!Role::Developer.has(Capability::DlqReplay));
        assert!(!Role::Developer.has(Capability::BroadcastCompose));

        // env:create, hmac:rotate, user:manage are admin-only.
        for cap in [
            Capability::EnvCreate,
            Capability::HmacRotate,
            Capability::UserManage,
        ] {
            for role in [Role::Viewer, Role::Operator, Role::Developer] {
                assert!(!role.has(cap), "{} should not have {}", role, cap.as_str());
            }
        }
    }
}
