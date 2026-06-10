//! Task 4: the merged two-source inbox — keyset pagination across interleaved
//! sources, watermark moves, broadcast read exceptions + GC, counts, ETag
//! movement on EVERY read-state mutation, the conditional-increment race,
//! preferences, and the EXPLAIN shape check.

mod support;

use std::time::Duration;

use serde_json::json;

const SUB: &str = "usr_inbox";

async fn etag(app: &support::TestApp, subscriber: &str) -> String {
    let res = app
        .client
        .get(format!("{}/v1/inbox/items", app.base))
        .headers(app.subscriber_headers(subscriber))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    res.headers()["etag"].to_str().unwrap().to_owned()
}

#[tokio::test]
async fn merged_list_interleaves_both_sources_under_keyset_pagination() {
    let app = support::spawn().await;
    // Force subscriber creation BEFORE the first broadcast (visibility rule).
    app.create_notification(SUB, "direct.0").await;
    let mut expected = vec!["direct.0".to_owned()];
    for i in 1..=4 {
        app.create_broadcast(&format!("bcast.{i}")).await;
        expected.push(format!("bcast.{i}"));
        app.create_notification(SUB, &format!("direct.{i}")).await;
        expected.push(format!("direct.{i}"));
    }
    expected.reverse(); // newest first

    // Paginate at limit=3 (interior page boundaries cross sources).
    let items = app.list_all_items(SUB, 3).await;
    let categories: Vec<String> = items
        .iter()
        .map(|i| i["category"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(categories, expected);

    // Source discriminator + TypeID prefix agree.
    for item in &items {
        let id = item["id"].as_str().unwrap();
        match item["source"].as_str().unwrap() {
            "notification" => assert!(id.starts_with("notif_"), "{id}"),
            "broadcast" => assert!(id.starts_with("bcast_"), "{id}"),
            other => panic!("unknown source {other}"),
        }
        assert_eq!(item["read"], false);
    }

    // occurred_at strictly descending with id tiebreak ⇒ total order.
    let stamps: Vec<&str> = items
        .iter()
        .map(|i| i["occurred_at"].as_str().unwrap())
        .collect();
    let mut sorted = stamps.clone();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(stamps, sorted);

    // A short page means the end: next_cursor null.
    let page = app.list_items(SUB).await;
    assert!(page["next_cursor"].is_null());
    assert_eq!(page["items"].as_array().unwrap().len(), 9);
}

#[tokio::test]
async fn broadcast_visibility_follows_subscriber_created_at() {
    let app = support::spawn().await;
    app.create_broadcast("before").await;
    app.create_notification("usr_new", "x").await; // subscriber born here
    app.create_broadcast("after").await;

    let cats: Vec<String> = app
        .list_all_items("usr_new", 10)
        .await
        .iter()
        .map(|i| i["category"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        cats,
        ["after", "x"],
        "no announcements from before you existed"
    );

    // Backdated import sees history.
    let res = app
        .client
        .put(format!("{}/v1/subscribers/usr_old", app.base))
        .bearer_auth(&app.env.api_key)
        .json(&json!({ "created_at": "2020-01-01T00:00:00Z" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let cats: Vec<String> = app
        .list_all_items("usr_old", 10)
        .await
        .iter()
        .map(|i| i["category"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(cats, ["after", "before"]);

    let (unread, _) = app.counts("usr_old").await;
    assert_eq!(
        unread, 2,
        "broadcast contribution respects the visibility window"
    );
}

#[tokio::test]
async fn mark_all_read_is_a_watermark_move_that_covers_both_sources() {
    let app = support::spawn().await;
    app.create_notification(SUB, "d1").await;
    app.create_broadcast("b1").await;
    app.create_notification(SUB, "d2").await;
    // Individual broadcast read first → exception row exists.
    let bcast_id = app
        .list_all_items(SUB, 10)
        .await
        .iter()
        .find(|i| i["source"] == "broadcast")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/broadcasts/{bcast_id}/read"))
        .await;
    assert_eq!(res.status(), 204);
    let exceptions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM broadcast_reads WHERE environment_id = $1")
            .bind(app.env.id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(exceptions, 1);

    let res = app.post_inbox(SUB, "/v1/inbox/read-all").await;
    assert_eq!(res.status(), 200);
    let counts: serde_json::Value = res.json().await.unwrap();
    assert_eq!(counts["unread"], 0);

    // Read state covers BOTH sources; no notification row was UPDATEd.
    let items = app.list_all_items(SUB, 10).await;
    assert!(items.iter().all(|i| i["read"] == true), "{items:?}");
    let read_at_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE environment_id = $1 AND read_at IS NOT NULL",
    )
    .bind(app.env.id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(
        read_at_rows, 0,
        "mark-all-read must never bulk-UPDATE notifications"
    );

    // Exception rows at/below the watermark are GC'd.
    let exceptions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM broadcast_reads WHERE environment_id = $1")
            .bind(app.env.id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(exceptions, 0, "broadcast_reads GC on watermark move");

    // New items after the move are unread again.
    app.create_notification(SUB, "d3").await;
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 1);
}

#[tokio::test]
async fn individual_reads_decrement_exactly_once_and_are_idempotent() {
    let app = support::spawn().await;
    app.create_notification(SUB, "a").await;
    app.create_notification(SUB, "b").await;
    let items = app.list_all_items(SUB, 10).await;
    let id = items[0]["id"].as_str().unwrap().to_owned();

    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/notifications/{id}/read"))
        .await;
    assert_eq!(res.status(), 204);
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 1);

    // Idempotent: second read does not double-decrement.
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/notifications/{id}/read"))
        .await;
    assert_eq!(res.status(), 204);
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 1);

    let read_flags: Vec<bool> = app
        .list_all_items(SUB, 10)
        .await
        .iter()
        .map(|i| i["read"].as_bool().unwrap())
        .collect();
    assert_eq!(read_flags, [true, false]);

    // Cross-subscriber and cross-source 404s.
    let res = app
        .post_inbox("usr_other", &format!("/v1/inbox/notifications/{id}/read"))
        .await;
    assert_eq!(res.status(), 404);
    let res = app
        .post_inbox(
            SUB,
            "/v1/inbox/notifications/notif_01h455vb4pex5vsknk084sn02q/read",
        )
        .await;
    assert_eq!(res.status(), 404);
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/broadcasts/{id}/read"))
        .await;
    assert_eq!(
        res.status(),
        404,
        "a notification TypeID is not a broadcast"
    );
}

#[tokio::test]
async fn broadcast_read_exceptions_count_and_gc_correctly() {
    let app = support::spawn().await;
    app.create_notification(SUB, "seed").await; // subscriber exists
    app.create_broadcast("b1").await;
    app.create_broadcast("b2").await;
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 3);

    let items = app.list_all_items(SUB, 10).await;
    let bcast_id = items.iter().find(|i| i["source"] == "broadcast").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/broadcasts/{bcast_id}/read"))
        .await;
    assert_eq!(res.status(), 204);
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 2, "exception row subtracts from the broadcast term");

    // Idempotent.
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/broadcasts/{bcast_id}/read"))
        .await;
    assert_eq!(res.status(), 204);
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 2);

    let read_flags: Vec<(String, bool)> = app
        .list_all_items(SUB, 10)
        .await
        .iter()
        .map(|i| {
            (
                i["id"].as_str().unwrap().to_owned(),
                i["read"].as_bool().unwrap(),
            )
        })
        .collect();
    assert!(read_flags.iter().any(|(id, read)| *id == bcast_id && *read));
    assert_eq!(read_flags.iter().filter(|(_, read)| *read).count(), 1);
}

#[tokio::test]
async fn seen_state_is_watermark_only_and_independent_of_read() {
    let app = support::spawn().await;
    app.create_notification(SUB, "a").await;
    app.create_broadcast("b").await;
    let (unread, unseen) = app.counts(SUB).await;
    assert_eq!((unread, unseen), (2, 2));

    let res = app.post_inbox(SUB, "/v1/inbox/seen-all").await;
    assert_eq!(res.status(), 200);
    let counts: serde_json::Value = res.json().await.unwrap();
    assert_eq!(counts["unseen"], 0);
    assert_eq!(counts["unread"], 2, "seen-all leaves read state untouched");

    app.create_notification(SUB, "c").await;
    let (unread, unseen) = app.counts(SUB).await;
    assert_eq!((unread, unseen), (3, 1));
}

#[tokio::test]
async fn etag_moves_on_every_read_state_mutation_and_serves_304s() {
    let app = support::spawn().await;
    app.create_notification(SUB, "x").await;
    let mut seen_etags = vec![etag(&app, SUB).await];
    let assert_moved = async |app: &support::TestApp, label: &str, seen: &mut Vec<String>| {
        let e = etag(app, SUB).await;
        assert!(!seen.contains(&e), "ETag did not move after {label}");
        seen.push(e);
    };

    app.create_notification(SUB, "y").await;
    assert_moved(&app, "create", &mut seen_etags).await;

    app.create_broadcast("b").await;
    assert_moved(&app, "broadcast create", &mut seen_etags).await;

    let items = app.list_all_items(SUB, 10).await;
    let notif_id = items
        .iter()
        .find(|i| i["source"] == "notification")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let bcast_id = items.iter().find(|i| i["source"] == "broadcast").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    app.post_inbox(SUB, &format!("/v1/inbox/notifications/{notif_id}/read"))
        .await;
    assert_moved(&app, "mark notification read", &mut seen_etags).await;

    // Mark-broadcast-read changes no maintained counter — the updated_at bump
    // is the only thing that moves the ETag here.
    app.post_inbox(SUB, &format!("/v1/inbox/broadcasts/{bcast_id}/read"))
        .await;
    assert_moved(&app, "mark broadcast read", &mut seen_etags).await;

    app.post_inbox(SUB, "/v1/inbox/read-all").await;
    assert_moved(&app, "read-all", &mut seen_etags).await;

    app.post_inbox(SUB, "/v1/inbox/seen-all").await;
    assert_moved(&app, "seen-all", &mut seen_etags).await;

    let res = app
        .client
        .put(format!("{}/v1/inbox/preferences", app.base))
        .headers(app.subscriber_headers(SUB))
        .json(&json!({ "preferences": [ { "category": "x", "channel": "in_app", "enabled": false } ] }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_moved(&app, "preference flip", &mut seen_etags).await;

    // Quiescent: If-None-Match → 304 with headers, no body.
    let current = seen_etags.last().unwrap().clone();
    let res = app
        .client
        .get(format!("{}/v1/inbox/items", app.base))
        .headers(app.subscriber_headers(SUB))
        .header("If-None-Match", &current)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 304);
    assert_eq!(res.headers()["etag"].to_str().unwrap(), current);
    assert_eq!(
        res.headers()["cache-control"].to_str().unwrap(),
        "private, max-age=0"
    );

    // …and a change flips it back to 200.
    app.create_notification(SUB, "z").await;
    let res = app
        .client
        .get(format!("{}/v1/inbox/items", app.base))
        .headers(app.subscriber_headers(SUB))
        .header("If-None-Match", &current)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

/// The conditional-increment race: mark-all-read concurrent with inserts must
/// never leave drift between the maintained counter and the truth.
#[tokio::test]
async fn concurrent_mark_all_read_and_inserts_leave_no_counter_drift() {
    let app = support::spawn().await;
    app.create_notification(SUB, "seed").await;

    for round in 0..15 {
        let create = {
            let client = app.client.clone();
            let base = app.base.clone();
            let key = app.env.api_key.clone();
            tokio::spawn(async move {
                let res = client
                    .post(format!("{base}/v1/notifications"))
                    .bearer_auth(key)
                    .json(&json!({ "subscriber_id": SUB, "category": format!("race.{round}") }))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(res.status(), 201);
            })
        };
        let read_all = {
            let client = app.client.clone();
            let base = app.base.clone();
            let headers = app.subscriber_headers(SUB);
            tokio::spawn(async move {
                let res = client
                    .post(format!("{base}/v1/inbox/read-all"))
                    .headers(headers)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(res.status(), 200);
            })
        };
        create.await.unwrap();
        read_all.await.unwrap();
    }

    // Invariant: maintained counter == recount from the rows, exactly.
    let (counter, truth): (i32, i64) = sqlx::query_as(
        "SELECT c.unread_direct_count,
                (SELECT count(*) FROM notifications n
                  WHERE n.environment_id = c.environment_id
                    AND n.subscriber_id = c.subscriber_id
                    AND n.visible_at <= now() AND n.read_at IS NULL
                    AND n.visible_at > c.read_watermark)
           FROM subscriber_counters c
           JOIN subscribers s ON s.environment_id = c.environment_id AND s.id = c.subscriber_id
          WHERE c.environment_id = $1 AND s.subscriber_id = $2",
    )
    .bind(app.env.id)
    .bind(SUB)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(
        i64::from(counter),
        truth,
        "permanent counter drift detected"
    );
}

#[tokio::test]
async fn muted_categories_disappear_from_the_list_at_read_time() {
    let app = support::spawn().await;
    app.create_notification(SUB, "noisy").await;
    app.create_notification(SUB, "important").await;
    app.create_broadcast("noisy").await;

    let res = app
        .client
        .put(format!("{}/v1/inbox/preferences", app.base))
        .headers(app.subscriber_headers(SUB))
        .json(&json!({ "preferences": [ { "category": "noisy", "channel": "in_app", "enabled": false } ] }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["preferences"][0]["category"], "noisy");
    assert_eq!(body["preferences"][0]["enabled"], false);

    let cats: Vec<String> = app
        .list_all_items(SUB, 10)
        .await
        .iter()
        .map(|i| i["category"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(cats, ["important"], "mutes filter BOTH sources");

    // enabled=true deletes the explicit row (absence means enabled).
    let res = app
        .client
        .put(format!("{}/v1/inbox/preferences", app.base))
        .headers(app.subscriber_headers(SUB))
        .json(&json!({ "preferences": [ { "category": "noisy", "channel": "in_app", "enabled": true } ] }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["preferences"].as_array().unwrap().len(), 0);
    assert_eq!(app.list_all_items(SUB, 10).await.len(), 3);

    // Unknown channels are an API-layer 400 (no DB CHECK by design).
    let res = app
        .client
        .put(format!("{}/v1/inbox/preferences", app.base))
        .headers(app.subscriber_headers(SUB))
        .json(&json!({ "preferences": [ { "category": "x", "channel": "web_push", "enabled": false } ] }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn management_preference_endpoints_mirror_the_subscriber_plane() {
    let app = support::spawn().await;

    // GET for an unknown subscriber is a 404, never a lazy create.
    let res = app
        .client
        .get(format!("{}/v1/subscribers/usr_ghost/preferences", app.base))
        .bearer_auth(&app.env.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);

    // PUT lazily creates and writes.
    let res = app
        .client
        .put(format!("{}/v1/subscribers/usr_admin/preferences", app.base))
        .bearer_auth(&app.env.api_key)
        .json(&json!({ "preferences": [ { "category": "noisy", "channel": "in_app", "enabled": false } ] }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    let res = app
        .client
        .get(format!("{}/v1/subscribers/usr_admin/preferences", app.base))
        .bearer_auth(&app.env.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["preferences"][0]["category"], "noisy");

    // The subscriber plane sees the same rows.
    let res = app
        .client
        .get(format!("{}/v1/inbox/preferences", app.base))
        .headers(app.subscriber_headers("usr_admin"))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["preferences"][0]["category"], "noisy");
}

/// Acceptance: EXPLAIN confirms notifications_inbox_idx and
/// broadcasts_window_idx serve the two merge arms.
#[tokio::test]
async fn explain_confirms_the_two_arm_index_shapes() {
    let app = support::spawn().await;
    app.create_notification(SUB, "x").await;
    app.create_broadcast("y").await;

    let mut conn = app.pool.acquire().await.unwrap();
    // Tiny tables would seq-scan; prove the indexes CAN serve the shapes.
    sqlx::query("SET enable_seqscan = off")
        .execute(&mut *conn)
        .await
        .unwrap();
    let plan_rows: Vec<String> = sqlx::query_scalar(
        r"EXPLAIN
          SELECT * FROM (
            (SELECT n.visible_at AS occurred_at, n.id FROM notifications n
              WHERE n.environment_id = $1 AND n.subscriber_id = $2
                AND n.visible_at <= now()
                AND (n.visible_at, n.id) < (now(), $3)
              ORDER BY n.visible_at DESC, n.id DESC LIMIT 20)
            UNION ALL
            (SELECT b.created_at, b.id FROM broadcasts b
              WHERE b.environment_id = $1
                AND b.created_at >= now() - interval '1 day'
                AND (b.created_at, b.id) < (now(), $3)
              ORDER BY b.created_at DESC, b.id DESC LIMIT 20)
          ) merged ORDER BY occurred_at DESC, id DESC LIMIT 20",
    )
    .bind(app.env.id)
    .bind(uuid::Uuid::nil())
    .bind(uuid::Uuid::max())
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    let plan = plan_rows.join("\n");
    // notifications_inbox_idx propagates to partitions under generated child
    // names (notifications_YYYY_MM_environment_id_subscriber_id_visible__idx).
    assert!(
        plan.contains("environment_id_subscriber_id_visible"),
        "direct arm not served by the inbox index:\n{plan}"
    );
    assert!(
        plan.contains("broadcasts_window_idx"),
        "broadcast arm not served by the window index:\n{plan}"
    );
}

#[tokio::test]
async fn cursor_pagination_is_stable_under_concurrent_inserts() {
    let app = support::spawn().await;
    for i in 0..6 {
        app.create_notification(SUB, &format!("c{i}")).await;
    }
    // First page…
    let res = app
        .client
        .get(format!("{}/v1/inbox/items?limit=3", app.base))
        .headers(app.subscriber_headers(SUB))
        .send()
        .await
        .unwrap();
    let page1: serde_json::Value = res.json().await.unwrap();
    let cursor = page1["next_cursor"].as_str().unwrap().to_owned();

    // …a newer item arrives between pages…
    app.create_notification(SUB, "newest").await;

    // …and the second page neither skips nor repeats (keyset, not offset).
    let res = app
        .client
        .get(format!(
            "{}/v1/inbox/items?limit=3&cursor={cursor}",
            app.base
        ))
        .headers(app.subscriber_headers(SUB))
        .send()
        .await
        .unwrap();
    let page2: serde_json::Value = res.json().await.unwrap();
    let cats: Vec<&str> = page2["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["category"].as_str().unwrap())
        .collect();
    assert_eq!(cats, ["c2", "c1", "c0"]);

    // Malformed cursor → 400, not 500.
    let res = app
        .client
        .get(format!("{}/v1/inbox/items?cursor=%21%21%21", app.base))
        .headers(app.subscriber_headers(SUB))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);

    tokio::time::sleep(Duration::from_millis(10)).await;
}
