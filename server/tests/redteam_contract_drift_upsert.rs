//! Guards annotation-vs-handler drift for `upsertSubscriber`. `PUT
//! /v1/subscribers/{subscriber_id}` returns 400 when `subscriber_id` is empty
//! or over 255 chars, so its `#[utoipa::path]` must declare 400. The contract
//! is code-first via utoipa, so the served spec is the handler's truth.

mod support;

use dronte::openapi::api_doc;

/// `upsertSubscriber` returns 400 for an over-long `subscriber_id`. The
/// code-first utoipa document must declare it.
#[tokio::test]
async fn upsert_subscriber_400_is_declared_in_the_generated_spec() {
    let app = support::spawn().await;

    // Auth passes, then the handler rejects the id with 400. The contract
    // limit is 1-255 chars.
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
