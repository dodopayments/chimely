//! Code-first OpenAPI document (utoipa).
//!
//! Contract rule (see CLAUDE.md): until v1, `specs/openapi.yaml` is the
//! convergence target. CI exports this document (`dronte openapi`) and runs
//! oasdiff against the spec; the diff is the to-do list. All free text below
//! is verbatim from the spec — the goal is an empty delta for implemented
//! surface.

use utoipa::OpenApi;
use utoipa::openapi::content::ContentBuilder;
use utoipa::openapi::header::HeaderBuilder;
use utoipa::openapi::response::ResponseBuilder;
use utoipa::openapi::schema::{ObjectBuilder, Type};
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, SecurityScheme};
use utoipa::openapi::{Ref, ServerBuilder};

/// Verbatim from specs/openapi.yaml `info.description`.
const INFO_DESCRIPTION: &str = r#"Two planes, one binary:

* **Management plane** — called by the customer's backend with a Bearer API
  key. Creates notifications and broadcasts, manages subscribers and their
  preferences. API keys are environment-scoped; the environment is implied
  by the key.
* **Subscriber plane** — called by `@dronte/client` (the `<Inbox />`
  widget) on behalf of one end user. Authenticated with an HMAC subscriber
  hash: `hex(HMAC-SHA256(environment.subscriber_hmac_secret, subscriber_id))`
  computed by the customer's backend. Mandatory in environments with
  `require_subscriber_hash = true` (the production default); optional in
  dev environments so the quickstart works without a backend.

Subscriber-plane scoping travels in headers (or query parameters where
headers are impossible, i.e. `EventSource`):

| Header | Query fallback | Meaning |
|---|---|---|
| `X-Dronte-Environment` | `environment` | environment slug |
| `X-Dronte-Subscriber` | `subscriber_id` | customer-provided subscriber id |
| `X-Dronte-Subscriber-Hash` | `subscriber_hash` | HMAC hash (when required) |

**Idempotency.** Every create accepts `idempotency_key` (client-supplied or
server-generated and echoed). Uniqueness is per environment per resource
type. A retried key returns the original response byte-identically with
HTTP 200 (first acceptance is 201) — acknowledged-and-dropped.

**Timestamps** are RFC 3339 UTC. **IDs** are
[TypeIDs](https://github.com/jetify-com/typeid): a resource prefix plus a
UUIDv7 in Crockford base32 — `notif_01h455vb4pex5vsknk084sn02q` for
notifications, `bcast_…` for broadcasts. The prefix is part of the id;
treat ids as opaque strings.

**Errors** use a single envelope: `{"error": {"code", "message"}}` with
conventional status codes. 429 carries `Retry-After`.
"#;

/// Title/version deliberately mirror specs/openapi.yaml so the oasdiff delta
/// is only the parts we haven't built yet, not metadata noise.
#[derive(OpenApi)]
#[openapi(
    info(title = "Dronte API", version = "1.0.0"),
    tags(
        (name = "management", description = "Backend-to-Dronte. Bearer API key."),
        (name = "subscriber", description = "Widget-to-Dronte. HMAC subscriber hash.")
    ),
    paths(
        crate::api::management::create_notifications,
        crate::api::management::create_broadcast,
        crate::api::management::upsert_subscriber,
        crate::api::preferences::get_subscriber_preferences,
        crate::api::preferences::set_subscriber_preferences,
        crate::api::inbox::list_items,
        crate::api::inbox::get_counts,
        crate::api::inbox::mark_notification_read,
        crate::api::inbox::mark_broadcast_read,
        crate::api::inbox::mark_all_read,
        crate::api::inbox::mark_all_seen,
        crate::api::preferences::get_inbox_preferences,
        crate::api::preferences::set_inbox_preferences,
        crate::api::sse::stream,
    ),
    components(schemas(
        crate::api::contract::NotificationId,
        crate::api::contract::BroadcastId,
        crate::api::contract::Payload,
        crate::api::contract::Error,
        crate::api::contract::CreateNotificationsRequest,
        crate::api::contract::CreateNotificationsResponse,
        crate::api::contract::CreateBroadcastRequest,
        crate::api::contract::Broadcast,
        crate::api::contract::Subscriber,
        crate::api::contract::InboxItem,
        crate::api::contract::InboxPage,
        crate::api::contract::InboxCounts,
        crate::api::contract::Preference,
        crate::api::contract::PreferenceList,
        crate::api::contract::PreferenceWriteList,
    ))
)]
pub struct ApiDoc;

pub fn api_doc() -> utoipa::openapi::OpenApi {
    let mut doc = ApiDoc::openapi();
    doc.info.description = Some(INFO_DESCRIPTION.to_owned());
    doc.info.license = None;
    doc.servers = Some(vec![
        ServerBuilder::new()
            .url("https://dronte.example.com")
            .build(),
    ]);
    // The spec's explicit top-level `security: []`.
    doc.security = Some(vec![]);

    let components = doc.components.get_or_insert_with(Default::default);
    components.add_security_scheme(
        "ApiKeyBearer",
        SecurityScheme::Http(
            utoipa::openapi::security::HttpBuilder::new()
                .scheme(HttpAuthScheme::Bearer)
                .description(Some("Environment-scoped management API key."))
                .build(),
        ),
    );
    components.add_security_scheme(
        "SubscriberEnv",
        SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("X-Dronte-Environment"))),
    );
    components.add_security_scheme(
        "SubscriberId",
        SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("X-Dronte-Subscriber"))),
    );
    components.add_security_scheme(
        "SubscriberHash",
        SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::with_description(
            "X-Dronte-Subscriber-Hash",
            "Optional when the environment has require_subscriber_hash = false.",
        ))),
    );
    components.add_security_scheme(
        "SubscriberEnvQ",
        SecurityScheme::ApiKey(ApiKey::Query(ApiKeyValue::new("environment"))),
    );
    components.add_security_scheme(
        "SubscriberIdQ",
        SecurityScheme::ApiKey(ApiKey::Query(ApiKeyValue::new("subscriber_id"))),
    );
    components.add_security_scheme(
        "SubscriberHashQ",
        SecurityScheme::ApiKey(ApiKey::Query(ApiKeyValue::new("subscriber_hash"))),
    );

    // Reusable error responses (components.responses). Paths inline
    // equivalent responses — references resolve identically; RateLimited is
    // declared now and wired to endpoints with Phase 3 rate limiting.
    let error_content = || {
        ContentBuilder::new()
            .schema(Some(Ref::from_schema_name("Error")))
            .build()
    };
    components.responses.insert(
        "BadRequest".to_owned(),
        ResponseBuilder::new()
            .description("Validation error.")
            .content("application/json", error_content())
            .build()
            .into(),
    );
    components.responses.insert(
        "Unauthorized".to_owned(),
        ResponseBuilder::new()
            .description("Missing/invalid API key or subscriber hash.")
            .content("application/json", error_content())
            .build()
            .into(),
    );
    components.responses.insert(
        "NotFound".to_owned(),
        ResponseBuilder::new()
            .description("Resource not found in this environment.")
            .content("application/json", error_content())
            .build()
            .into(),
    );
    components.responses.insert(
        "RateLimited".to_owned(),
        ResponseBuilder::new()
            .description("Per-API-key (management) or per-subscriber (widget) rate limit.")
            // lowercase: kin-openapi (the oasdiff loader) normalizes header
            // names to lowercase; matching avoids a phantom casing diff.
            .header(
                "retry-after",
                HeaderBuilder::new()
                    .schema(ObjectBuilder::new().schema_type(Type::Integer))
                    .build(),
            )
            .content("application/json", error_content())
            .build()
            .into(),
    );

    fixups(&mut doc);
    doc
}

/// Surgical overrides where utoipa's macro surface cannot express the frozen
/// 3.0 contract: plain-`integer` query params with defaults, non-nullable
/// optional header params, and an optional request body. Everything here is
/// still code-first. It edits the generated document object, never the
/// serialized output.
fn fixups(doc: &mut utoipa::openapi::OpenApi) {
    use utoipa::openapi::Required;

    let plain_string = || {
        ObjectBuilder::new()
            .schema_type(Type::String)
            .build()
            .into()
    };

    if let Some(item) = doc.paths.paths.get_mut("/v1/inbox/items")
        && let Some(get) = item.get.as_mut()
        && let Some(params) = get.parameters.as_mut()
    {
        for param in params.iter_mut() {
            match param.name.as_str() {
                // Option<i32> renders int32-formatted; the contract wants a
                // plain integer with bounds and a default.
                "limit" => {
                    param.schema = Some(
                        ObjectBuilder::new()
                            .schema_type(Type::Integer)
                            .minimum(Some(1))
                            .maximum(Some(100))
                            .default(Some(serde_json::json!(20)))
                            .build()
                            .into(),
                    );
                }
                // Optional params are `required: false`, not nullable.
                "If-None-Match" => param.schema = Some(plain_string()),
                _ => {}
            }
        }
    }

    if let Some(item) = doc.paths.paths.get_mut("/v1/inbox/stream")
        && let Some(get) = item.get.as_mut()
        && let Some(params) = get.parameters.as_mut()
    {
        for param in params.iter_mut() {
            if param.name == "Last-Event-ID" {
                param.schema = Some(plain_string());
            }
        }
    }

    // The upsert body is optional in the contract.
    if let Some(item) = doc.paths.paths.get_mut("/v1/subscribers/{subscriber_id}")
        && let Some(put) = item.put.as_mut()
        && let Some(body) = put.request_body.as_mut()
    {
        body.required = Some(Required::False);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_yaml_with_expected_identity() {
        let yaml = api_doc().to_yaml().expect("spec serializes");
        assert!(yaml.contains("title: Dronte API"));
        assert!(yaml.contains("version: 1.0.0"));
    }

    #[test]
    fn declares_both_planes_as_tags() {
        let doc = api_doc();
        let tags: Vec<String> = doc
            .tags
            .unwrap_or_default()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(tags, ["management", "subscriber"]);
    }

    #[test]
    fn declares_every_v1_path() {
        let doc = api_doc();
        for path in [
            "/v1/notifications",
            "/v1/broadcasts",
            "/v1/subscribers/{subscriber_id}",
            "/v1/subscribers/{subscriber_id}/preferences",
            "/v1/inbox/items",
            "/v1/inbox/counts",
            "/v1/inbox/notifications/{id}/read",
            "/v1/inbox/broadcasts/{id}/read",
            "/v1/inbox/read-all",
            "/v1/inbox/seen-all",
            "/v1/inbox/preferences",
            "/v1/inbox/stream",
        ] {
            assert!(doc.paths.paths.contains_key(path), "missing path: {path}");
        }
    }
}
