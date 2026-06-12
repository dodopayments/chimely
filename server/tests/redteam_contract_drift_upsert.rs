//! REGRESSION GUARD (was a RED-TEAM finding): the `upsertSubscriber` utoipa
//! annotation now tells the truth about its status codes. `PUT
//! /v1/subscribers/{subscriber_id}` returns HTTP 400 from its own request
//! validation (`subscriber_id` empty or > 255 chars,
//! management.rs::upsert_subscriber); the `#[utoipa::path]` now DECLARES that
//! 400 alongside 200/401, so the generated/served OpenAPI document matches the
//! handler, `@dronte/client` gets a 400 branch, and a schemathesis run that
//! generates an over-long path segment no longer reports an undocumented status.
//! Before the fix the annotation declared only 200 and 401.
//!
//! CONTRACT RULE under attack (CLAUDE.md): "The contract is code-first via
//! utoipa" and "A light schemathesis run guards against annotation-vs-handler
//! drift (utoipa response codes are hand-annotated)." The companion guard
//! `redteam_contract_drift.rs` pins the SAME class of bug for
//! `listInboxItems` (whose 400 was added); this is the unfixed sibling. The
//! param even declares `max_length = 255`, which makes the missing 400 response
//! a self-contradiction: the spec says the input can be rejected but never
//! lists the rejection's status.
//!
//! schemathesis is not installed in this environment and a full negative-test
//! run needs a stood-up live instance with a seeded API key, so this reproduces
//! the single check that matters without it (the same approach
//! redteam_contract_drift.rs takes): the status the handler really returns is
//! NOT the status the annotation declares.

mod support;

use dronte::openapi::api_doc;

/// `upsertSubscriber` returns 400 for an over-long `subscriber_id`, but the
/// code-first utoipa document does not declare it.
#[tokio::test]
async fn upsert_subscriber_400_is_declared_in_the_generated_spec() {
    let app = support::spawn().await;

    // Behaviour: a valid management key, a 256-character subscriber id in the
    // path. Auth passes; the handler rejects the id with 400 (the contract
    // limit is 1-255).
    let overlong = "u".repeat(256);
    let res = app
        .client
        .put(format!("{}/v1/subscribers/{overlong}", app.base))
        .bearer_auth(&app.env.api_key)
        .send()
        .await
        .expect("upsert subscriber request");
    assert_eq!(
        res.status(),
        400,
        "handler rejects a >255-char subscriber_id with 400"
    );

    // Annotation: what the code-first utoipa document actually declares for the
    // PUT operation.
    let doc = api_doc();
    let op = doc
        .paths
        .paths
        .get("/v1/subscribers/{subscriber_id}")
        .expect("path present")
        .put
        .as_ref()
        .expect("PUT operation present");
    let declared: Vec<String> = op.responses.responses.keys().cloned().collect();

    assert!(
        op.responses.responses.contains_key("400"),
        "CONTRACT DRIFT: the upsertSubscriber annotation must declare the 400 the \
         handler returns for an out-of-range subscriber_id; the generated spec declares \
         only {declared:?}. @dronte/client is built without a 400 branch and schemathesis \
         would report an undocumented HTTP status code.",
    );
}
