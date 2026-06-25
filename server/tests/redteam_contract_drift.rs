//! utoipa response codes are hand-annotated, so a handler can return a status
//! its annotation never declares. This asserts the status the handler returns
//! is the status the annotation declares.
//!
//! `GET /v1/inbox/items` returns 400 from request validation (out-of-range
//! `limit`, malformed `cursor`) and from the query extractor (non-integer
//! `limit`). The generated OpenAPI document must declare that 400 so it
//! matches the handler and `@chimely/client` has a 400 branch.

mod support;

use chimely::openapi::api_doc;

/// `listInboxItems` returns 400 for bad input, and the annotation declares it.
#[tokio::test]
async fn list_items_400_is_declared_in_the_generated_spec() {
    let app = support::spawn().await;

    let res = app
        .client
        .get(format!("{}/v1/inbox/items?limit=0", app.base))
        .headers(app.subscriber_headers("usr_contract"))
        .send()
        .await
        .expect("list items request");
    assert_eq!(res.status(), 400, "handler rejects limit=0 with 400");

    let doc = api_doc();
    let op = doc
        .paths
        .paths
        .get("/v1/inbox/items")
        .expect("path present")
        .get
        .as_ref()
        .expect("GET operation present");
    let declared: Vec<String> = op.responses.responses.keys().cloned().collect();

    assert!(
        op.responses.responses.contains_key("400"),
        "utoipa annotation must declare the 400 the handler returns for a bad \
         limit/cursor; generated spec declares only {declared:?}. schemathesis \
         would otherwise report an undocumented HTTP status code.",
    );
}
