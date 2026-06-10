//! Doc-only schema definitions converging the exported OpenAPI document on
//! specs/openapi.yaml (the frozen contract). Built with utoipa builders so
//! every description, constraint, and example is verbatim; the runtime
//! request/response types live next to their handlers and the schemathesis
//! CI job guards against drift between the two.

// The frozen contract is OpenAPI 3.0.3, which uses the singular `example`
// keyword; utoipa deprecates it in favor of 3.1 `examples`. Converging on the
// spec wins until v1.
#![allow(deprecated)]

use std::borrow::Cow;

use serde_json::json;
use utoipa::openapi::schema::{
    AdditionalProperties, ArrayBuilder, ObjectBuilder, OneOfBuilder, Schema, SchemaFormat,
    SchemaType, Type,
};
use utoipa::openapi::{Ref, RefOr};
use utoipa::{PartialSchema, ToSchema};

fn string() -> ObjectBuilder {
    ObjectBuilder::new().schema_type(Type::String)
}

fn date_time() -> ObjectBuilder {
    string().format(Some(SchemaFormat::Custom("date-time".into())))
}

fn uri() -> ObjectBuilder {
    string().format(Some(SchemaFormat::Custom("uri".into())))
}

macro_rules! contract_schema {
    ($name:ident, $build:expr) => {
        pub struct $name;
        impl PartialSchema for $name {
            fn schema() -> RefOr<Schema> {
                $build
            }
        }
        impl ToSchema for $name {
            fn name() -> Cow<'static, str> {
                Cow::Borrowed(stringify!($name))
            }
        }
    };
}

contract_schema!(
    NotificationId,
    string()
        .pattern(Some("^notif_[0-7][0-9a-hjkmnp-tv-z]{25}$"))
        .example(Some(json!("notif_01h455vb4pex5vsknk084sn02q")))
        .description(Some("TypeID: `notif_` + UUIDv7 suffix (Crockford base32)."))
        .into()
);

contract_schema!(
    BroadcastId,
    string()
        .pattern(Some("^bcast_[0-7][0-9a-hjkmnp-tv-z]{25}$"))
        .example(Some(json!("bcast_01h455vb4pex5vsknk084sn02q")))
        .description(Some("TypeID: `bcast_` + UUIDv7 suffix (Crockford base32)."))
        .into()
);

contract_schema!(
    Payload,
    ObjectBuilder::new()
        .description(Some(
            r#"Customer-defined JSON, delivered to the widget verbatim — the server
never reads it (no templates, no interpretation). Max 16 KiB serialized.

The optional **well-known fields** below are what the default
`<Inbox />` rendering understands; a payload with none of them renders
as category + timestamp only. All other fields ride along untouched
for custom renderers (`renderItem`). Keys are snake_case: payloads are
wire format and are never case-transformed by the SDK.
"#,
        ))
        .property(
            "title",
            string().description(Some("First line of the default item rendering."))
        )
        .property(
            "body",
            string().description(Some("Secondary line; treated as plain text, never HTML.")),
        )
        .property(
            "action_url",
            uri().description(Some(
                "Followed on item click by the default renderer (after mark-read).",
            )),
        )
        .property(
            "icon_url",
            uri().description(Some("Leading icon/avatar in the default rendering.")),
        )
        .additional_properties(Some(AdditionalProperties::FreeForm(true)))
        .into()
);

contract_schema!(
    Error,
    ObjectBuilder::new()
        .property(
            "error",
            ObjectBuilder::new()
                .property("code", string().example(Some(json!("invalid_request"))))
                .property("message", string())
                .required("code")
                .required("message"),
        )
        .required("error")
        .into()
);

contract_schema!(
    CreateNotificationsRequest,
    ObjectBuilder::new()
        .description(Some(
            "Exactly one of `subscriber_id` / `subscriber_ids` is required."
        ))
        .property(
            "subscriber_id",
            string()
                .max_length(Some(255))
                .description(Some("Single-recipient sugar for `subscriber_ids: [x]`.")),
        )
        .property(
            "subscriber_ids",
            ArrayBuilder::new()
                .items(string().max_length(Some(255)))
                .min_items(Some(1))
                .max_items(Some(100))
                .description(Some(
                    "Recipients; one notification row each. >100 → use a broadcast.",
                )),
        )
        .property(
            "category",
            string()
                .max_length(Some(255))
                .example(Some(json!("payment.failed"))),
        )
        .property("payload", Ref::from_schema_name("Payload"))
        .property(
            "idempotency_key",
            string().max_length(Some(255)).description(Some(
                "Client-supplied; server-generated and echoed if omitted. Covers the whole batch.",
            )),
        )
        .property(
            "deliver_at",
            date_time().description(Some(
                "Scheduled delivery; must be in the future, at most 13 months out.",
            )),
        )
        .required("category")
        .into()
);

contract_schema!(
    CreateNotificationsResponse,
    ObjectBuilder::new()
        .property("idempotency_key", string())
        .property(
            "notifications",
            ArrayBuilder::new().items(
                ObjectBuilder::new()
                    .property("id", Ref::from_schema_name("NotificationId"))
                    .property("subscriber_id", string())
                    .required("id")
                    .required("subscriber_id"),
            ),
        )
        .required("idempotency_key")
        .required("notifications")
        .into()
);

contract_schema!(
    CreateBroadcastRequest,
    ObjectBuilder::new()
        .property(
            "category",
            string()
                .max_length(Some(255))
                .example(Some(json!("product.update"))),
        )
        .property("payload", Ref::from_schema_name("Payload"))
        .property("idempotency_key", string().max_length(Some(255)))
        .required("category")
        .into()
);

contract_schema!(
    Broadcast,
    ObjectBuilder::new()
        .property("id", Ref::from_schema_name("BroadcastId"))
        .property("category", string())
        .property("payload", Ref::from_schema_name("Payload"))
        .property("created_at", date_time())
        .property("idempotency_key", string())
        .required("id")
        .required("category")
        .required("payload")
        .required("created_at")
        .required("idempotency_key")
        .into()
);

contract_schema!(
    Subscriber,
    ObjectBuilder::new()
        .property("subscriber_id", string())
        .property(
            "created_at",
            date_time().description(Some("Governs broadcast visibility for this subscriber.")),
        )
        .required("subscriber_id")
        .required("created_at")
        .into()
);

contract_schema!(
    InboxItem,
    ObjectBuilder::new()
        .property(
            "id",
            OneOfBuilder::new()
                .item(Ref::from_schema_name("NotificationId"))
                .item(Ref::from_schema_name("BroadcastId"))
                .description(Some(
                    "The TypeID prefix already encodes the source; `source` is kept as the explicit discriminator.",
                )),
        )
        .property(
            "source",
            string()
                .enum_values(Some(["notification", "broadcast"]))
                .description(Some(
                    "Routes mark-read to the right endpoint; the SDK handles this.",
                )),
        )
        .property("category", string())
        .property("payload", Ref::from_schema_name("Payload"))
        .property(
            "occurred_at",
            date_time().description(Some(
                "The ordering timestamp — `visible_at` for direct, `created_at` for broadcast.",
            )),
        )
        .property(
            "read",
            ObjectBuilder::new().schema_type(Type::Boolean).description(Some(
                "Computed — per-item exception OR at-or-below the read watermark.",
            )),
        )
        .required("id")
        .required("source")
        .required("category")
        .required("payload")
        .required("occurred_at")
        .required("read")
        .into()
);

contract_schema!(
    InboxPage,
    ObjectBuilder::new()
        .property(
            "items",
            ArrayBuilder::new().items(Ref::from_schema_name("InboxItem")),
        )
        .property(
            "next_cursor",
            string()
                .schema_type(SchemaType::from_iter([Type::String, Type::Null]))
                .description(Some(
                    "Opaque keyset token; null when the last page is reached."
                )),
        )
        .required("items")
        .required("next_cursor")
        .into()
);

contract_schema!(
    InboxCounts,
    ObjectBuilder::new()
        .property(
            "unread",
            ObjectBuilder::new()
                .schema_type(Type::Integer)
                .minimum(Some(0)),
        )
        .property(
            "unseen",
            ObjectBuilder::new()
                .schema_type(Type::Integer)
                .minimum(Some(0)),
        )
        .required("unread")
        .required("unseen")
        .into()
);

contract_schema!(
    Preference,
    ObjectBuilder::new()
        .property("category", string())
        .property(
            "channel",
            string().enum_values(Some(["in_app"])).description(Some(
                "Only `in_app` in v1; push channels later, no contract break.",
            )),
        )
        .property("enabled", ObjectBuilder::new().schema_type(Type::Boolean))
        .required("category")
        .required("channel")
        .required("enabled")
        .into()
);

contract_schema!(
    PreferenceList,
    ObjectBuilder::new()
        .property(
            "preferences",
            ArrayBuilder::new().items(Ref::from_schema_name("Preference")),
        )
        .required("preferences")
        .into()
);

contract_schema!(
    SseEventStream,
    string()
        .example(Some(json!(
            "id: 01h455vb4pex5vsknk084sn02q\nevent: hint\ndata: {\"reason\":\"notification\"}\n"
        )))
        .into()
);

contract_schema!(
    PreferenceWriteList,
    ObjectBuilder::new()
        .description(Some(
            "Partial upsert — listed (category, channel) pairs are written; unlisted pairs are untouched. Setting enabled=true deletes the explicit row (absence means enabled).",
        ))
        .property(
            "preferences",
            ArrayBuilder::new()
                .items(Ref::from_schema_name("Preference"))
                .min_items(Some(1))
                .max_items(Some(100)),
        )
        .required("preferences")
        .into()
);
