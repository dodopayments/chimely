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

fn declared(path: &str, method: &str) -> Vec<String> {
    let doc = api_doc();
    let item = doc
        .paths
        .paths
        .get(path)
        .unwrap_or_else(|| panic!("{path} missing from the spec"));
    let op = match method {
        "get" => item.get.as_ref(),
        "post" => item.post.as_ref(),
        "patch" => item.patch.as_ref(),
        _ => None,
    }
    .unwrap_or_else(|| panic!("{method} {path} missing from the spec"));
    op.responses.responses.keys().cloned().collect()
}

/// The subscriber-notification list, the move, and the environment PATCH
/// return only statuses that their annotations declare.
#[tokio::test]
async fn subscriber_move_list_and_environment_patch_statuses_are_declared() {
    let app = support::spawn().await;
    let list = "/v1/subscribers/{subscriber_id}/notifications";
    let mv = "/v1/subscribers/{subscriber_id}/notifications/move";
    let env_path = "/admin/api/environments/{env_id}";

    let mut seen: Vec<(&str, &str, u16)> = Vec::new();
    let get = |q: &str, key: bool| {
        let req = app.client.get(format!(
            "{}/v1/subscribers/usr_c/notifications{q}",
            app.base
        ));
        if key {
            req.bearer_auth(&app.env.api_key)
        } else {
            req
        }
    };
    seen.push((
        list,
        "get",
        get("", true).send().await.unwrap().status().as_u16(),
    ));
    seen.push((
        list,
        "get",
        get("?limit=0", true)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
    ));
    seen.push((
        list,
        "get",
        get("", false).send().await.unwrap().status().as_u16(),
    ));

    let body = serde_json::json!({ "to_subscriber_id": "usr_d", "notification_ids": [] });
    let same = serde_json::json!({ "to_subscriber_id": "usr_c", "notification_ids": [] });
    let url = format!("{}/v1/subscribers/usr_c/notifications/move", app.base);
    for (json, key) in [(&body, true), (&same, true), (&body, false)] {
        let req = app.client.post(&url).json(json);
        let req = if key {
            req.bearer_auth(&app.env.api_key)
        } else {
            req
        };
        seen.push((mv, "post", req.send().await.unwrap().status().as_u16()));
    }

    let env = chimely::ids::typeid(chimely::ids::ENVIRONMENT, app.env.id);
    let missing = chimely::ids::typeid(chimely::ids::ENVIRONMENT, uuid::Uuid::now_v7());
    for (id, json) in [
        (
            &env,
            serde_json::json!({ "subscriber_hash_max_ttl_seconds": 7200 }),
        ),
        (
            &env,
            serde_json::json!({ "subscriber_hash_max_ttl_seconds": 1 }),
        ),
        (
            &missing,
            serde_json::json!({ "require_subscriber_hash_expiry": true }),
        ),
    ] {
        let res = app
            .admin_patch(&format!("/admin/api/environments/{id}"), json)
            .send()
            .await
            .unwrap();
        seen.push((env_path, "patch", res.status().as_u16()));
    }
    let anon = reqwest::Client::new()
        .patch(format!("{}/admin/api/environments/{env}", app.base))
        .header("x-chimely-admin", "1")
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    seen.push((env_path, "patch", anon.status().as_u16()));

    let expected = [200, 400, 401, 200, 400, 401, 200, 400, 404, 401];
    let got: Vec<u16> = seen.iter().map(|s| s.2).collect();
    assert_eq!(got, expected, "handler statuses");
    for (path, method, status) in seen {
        let declared = declared(path, method);
        assert!(
            declared.contains(&status.to_string()),
            "{method} {path} returned {status} but declares only {declared:?}"
        );
    }
}
