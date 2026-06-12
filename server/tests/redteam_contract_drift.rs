//! Regression guard for the API contract rule (CLAUDE.md: "utoipa response
//! codes are hand-annotated"; "A light schemathesis run guards against
//! annotation-vs-handler drift").
//!
//! `GET /v1/inbox/items` returns HTTP 400 from its own request validation
//! (out-of-range `limit`, malformed `cursor`) and from the query extractor
//! (non-integer `limit`). The generated/served OpenAPI document must DECLARE
//! that 400 so it matches the handler and `@dronte/client` has a 400 branch.
//! The frozen 3.0 spec under-declares it, so the contract CI job
//! sanction-strips the 400 before the oasdiff convergence check (see ci.yml);
//! the generated spec served at /docs keeps it.
//!
//! schemathesis is not installed here and a full negative-test run needs a
//! stood-up live instance, so this reproduces the single check that matters:
//! the status the handler really returns is the status the annotation declares.

mod support;

use dronte::openapi::api_doc;

/// `listInboxItems` returns 400 for bad input, and the annotation declares it.
#[tokio::test]
async fn list_items_400_is_declared_in_the_generated_spec() {
    let app = support::spawn().await;

    // Behaviour: a valid subscriber, an invalid `limit`. The handler rejects
    // it with 400, past auth and the rate limiter.
    let res = app
        .client
        .get(format!("{}/v1/inbox/items?limit=0", app.base))
        .headers(app.subscriber_headers("usr_contract"))
        .send()
        .await
        .expect("list items request");
    assert_eq!(res.status(), 400, "handler rejects limit=0 with 400");

    // Annotation: what the code-first utoipa document actually declares.
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
