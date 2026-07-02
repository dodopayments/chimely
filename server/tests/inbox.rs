//! The merged two-source inbox. Keyset pagination across interleaved sources,
//! watermark moves, broadcast read exceptions and GC, counts, ETag movement on
//! every read-state mutation, the conditional-increment race, preferences, and
//! the EXPLAIN shape check.

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
    // Subscriber must exist before the first broadcast to be visible to it.
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

    // occurred_at strictly descending with id tiebreak gives a total order.
    let stamps: Vec<&str> = items
        .iter()
        .map(|i| i["occurred_at"].as_str().unwrap())
        .collect();
    let mut sorted = stamps.clone();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(stamps, sorted);

    // A short page is the last page. next_cursor is null.
    let page = app.list_items(SUB).await;
    assert!(page["next_cursor"].is_null());
    assert_eq!(page["items"].as_array().unwrap().len(), 9);
}

#[tokio::test]
async fn broadcast_visibility_follows_subscriber_created_at() {
    let app = support::spawn().await;
    app.create_broadcast("before").await;
    app.create_notification("usr_new", "x").await; // subscriber created here
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
    // Read a broadcast individually first so an exception row exists.
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

    // Read state covers both sources. No notification row was UPDATEd.
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

    // Marking a broadcast read changes no maintained counter. The updated_at
    // bump is the only thing that moves the ETag here.
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

    // Quiescent: If-None-Match yields 304 with headers and no body.
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

    // A change flips it back to 200.
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

/// EXPLAIN confirms notifications_inbox_idx and broadcasts_window_idx serve
/// the two merge arms.
#[tokio::test]
async fn explain_confirms_the_two_arm_index_shapes() {
    let app = support::spawn().await;
    app.create_notification(SUB, "x").await;
    app.create_broadcast("y").await;

    let mut conn = app.pool.acquire().await.unwrap();
    // Tiny tables would seq-scan. Disabling it forces the index shapes.
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
    let res = app
        .client
        .get(format!("{}/v1/inbox/items?limit=3", app.base))
        .headers(app.subscriber_headers(SUB))
        .send()
        .await
        .unwrap();
    let page1: serde_json::Value = res.json().await.unwrap();
    let cursor = page1["next_cursor"].as_str().unwrap().to_owned();

    // A newer item arrives between pages.
    app.create_notification(SUB, "newest").await;

    // The second page neither skips nor repeats. Keyset, not offset.
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

    // Malformed cursor is a 400, not a 500.
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

async fn list_filtered(
    app: &support::TestApp,
    subscriber: &str,
    filter: &str,
) -> Vec<serde_json::Value> {
    let mut items = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut url = format!("{}/v1/inbox/items?limit=2&filter={filter}", app.base);
        if let Some(c) = &cursor {
            url.push_str(&format!("&cursor={c}"));
        }
        let res = app
            .client
            .get(url)
            .headers(app.subscriber_headers(subscriber))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let page: serde_json::Value = res.json().await.unwrap();
        items.extend(page["items"].as_array().unwrap().clone());
        match page["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => return items,
        }
    }
}

#[tokio::test]
async fn mark_unread_survives_the_watermark_and_feeds_the_unread_view() {
    let app = support::spawn().await;
    app.create_notification(SUB, "a").await;
    app.create_notification(SUB, "b").await;
    app.create_notification(SUB, "c").await;
    let items = app.list_all_items(SUB, 10).await;
    let target = items[1]["id"].as_str().unwrap().to_owned();

    // Everything watermark-read, counter zero.
    let res = app.post_inbox(SUB, "/v1/inbox/read-all").await;
    assert_eq!(res.status(), 200);
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 0);

    // The override outranks the watermark: counted, unread in the list, and
    // the only row in the unread view. Tiny pages exercise the filtered
    // keyset.
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/notifications/{target}/unread"))
        .await;
    assert_eq!(res.status(), 204);
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 1);
    let unread_view = list_filtered(&app, SUB, "unread").await;
    assert_eq!(unread_view.len(), 1);
    assert_eq!(unread_view[0]["id"].as_str().unwrap(), target);
    assert!(!unread_view[0]["read"].as_bool().unwrap());

    // Idempotent: a second unread does not double-increment.
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/notifications/{target}/unread"))
        .await;
    assert_eq!(res.status(), 204);
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 1);

    // Read again clears the override and the count.
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/notifications/{target}/read"))
        .await;
    assert_eq!(res.status(), 204);
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 0);
    assert!(list_filtered(&app, SUB, "unread").await.is_empty());

    // A later read-all clears any remaining override state.
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/notifications/{target}/unread"))
        .await;
    assert_eq!(res.status(), 204);
    let res = app.post_inbox(SUB, "/v1/inbox/read-all").await;
    assert_eq!(res.status(), 200);
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 0);
    let overrides: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications
          WHERE environment_id = $1 AND unread_at IS NOT NULL",
    )
    .bind(app.env.id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(overrides, 0, "read-all GC'd the direct override");
}

#[tokio::test]
async fn broadcast_unread_overrides_round_trip_across_the_watermark() {
    let app = support::spawn().await;
    let res = app
        .client
        .put(format!("{}/v1/subscribers/{SUB}", app.base))
        .bearer_auth(&app.env.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let created = app.create_broadcast("announce").await;
    let id = created["id"].as_str().unwrap().to_owned();

    // Watermark-read, then explicitly unread below the watermark.
    let res = app.post_inbox(SUB, "/v1/inbox/read-all").await;
    assert_eq!(res.status(), 200);
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/broadcasts/{id}/unread"))
        .await;
    assert_eq!(res.status(), 204);
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 1, "broadcast override term counts it");
    let unread_view = list_filtered(&app, SUB, "unread").await;
    assert_eq!(unread_view.len(), 1);
    assert_eq!(unread_view[0]["id"].as_str().unwrap(), id);

    // Individual read below the watermark deletes the override (previously a
    // no-op branch).
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/broadcasts/{id}/read"))
        .await;
    assert_eq!(res.status(), 204);
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 0);
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM broadcast_reads WHERE environment_id = $1")
            .bind(app.env.id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(rows, 0, "the override row was deleted, not flipped");

    // Above the watermark: unread deletes a read row instead of writing one.
    let created = app.create_broadcast("announce2").await;
    let id2 = created["id"].as_str().unwrap().to_owned();
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/broadcasts/{id2}/read"))
        .await;
    assert_eq!(res.status(), 204);
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 0);
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/broadcasts/{id2}/unread"))
        .await;
    assert_eq!(res.status(), 204);
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 1);
}

#[tokio::test]
async fn unread_filter_rejects_unknown_values_and_moves_the_etag() {
    let app = support::spawn().await;
    app.create_notification(SUB, "a").await;

    let res = app
        .client
        .get(format!("{}/v1/inbox/items?filter=bogus", app.base))
        .headers(app.subscriber_headers(SUB))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);

    // The filter is an ETag input: the same state serves different
    // validators per view.
    let default_etag = etag(&app, SUB).await;
    let res = app
        .client
        .get(format!("{}/v1/inbox/items?filter=unread", app.base))
        .headers(app.subscriber_headers(SUB))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let unread_etag = res.headers()["etag"].to_str().unwrap().to_owned();
    assert_ne!(default_etag, unread_etag);

    // Unread mutations move the ETag.
    let items = app.list_all_items(SUB, 10).await;
    let id = items[0]["id"].as_str().unwrap().to_owned();
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/notifications/{id}/read"))
        .await;
    assert_eq!(res.status(), 204);
    let after_read = etag(&app, SUB).await;
    assert_ne!(default_etag, after_read);
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/notifications/{id}/unread"))
        .await;
    assert_eq!(res.status(), 204);
    let after_unread = etag(&app, SUB).await;
    assert_ne!(after_read, after_unread);
}

#[tokio::test]
async fn archive_round_trips_across_the_watermark_without_touching_read_state() {
    let app = support::spawn().await;
    app.create_notification(SUB, "a").await;
    app.create_notification(SUB, "b").await;
    let items = app.list_all_items(SUB, 10).await;
    let target = items[0]["id"].as_str().unwrap().to_owned();

    // Archiving an unread item removes it from the default view AND the
    // count, read state untouched.
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/notifications/{target}/archive"))
        .await;
    assert_eq!(res.status(), 204);
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 1);
    let default_view = app.list_all_items(SUB, 10).await;
    assert_eq!(default_view.len(), 1);
    let archived_view = list_filtered(&app, SUB, "archived").await;
    assert_eq!(archived_view.len(), 1);
    assert_eq!(archived_view[0]["id"].as_str().unwrap(), target);
    assert!(!archived_view[0]["read"].as_bool().unwrap(), "still unread");

    // Unarchive restores it, still unread, counted again. Idempotent.
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/notifications/{target}/unarchive"))
        .await;
    assert_eq!(res.status(), 204);
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/notifications/{target}/unarchive"))
        .await;
    assert_eq!(res.status(), 204);
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 2);
    assert_eq!(app.list_all_items(SUB, 10).await.len(), 2);

    // Archive-all: default view empties, counter zeroes, archived view has
    // everything. Read state still untouched.
    let res = app.post_inbox(SUB, "/v1/inbox/archive-all").await;
    assert_eq!(res.status(), 200);
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 0);
    assert!(app.list_all_items(SUB, 10).await.is_empty());
    let archived_view = list_filtered(&app, SUB, "archived").await;
    assert_eq!(archived_view.len(), 2);
    assert!(archived_view.iter().all(|i| !i["read"].as_bool().unwrap()));

    // Unarchive below the archive watermark: the override survives it and
    // the unread item re-enters the count.
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/notifications/{target}/unarchive"))
        .await;
    assert_eq!(res.status(), 204);
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 1);
    let default_view = app.list_all_items(SUB, 10).await;
    assert_eq!(default_view.len(), 1);
    assert_eq!(default_view[0]["id"].as_str().unwrap(), target);
}

#[tokio::test]
async fn broadcast_archive_overrides_round_trip_and_read_state_is_independent() {
    let app = support::spawn().await;
    let res = app
        .client
        .put(format!("{}/v1/subscribers/{SUB}", app.base))
        .bearer_auth(&app.env.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let created = app.create_broadcast("announce").await;
    let id = created["id"].as_str().unwrap().to_owned();

    // Archive above the watermark: override row, out of the default view
    // and the count.
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/broadcasts/{id}/archive"))
        .await;
    assert_eq!(res.status(), 204);
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 0);
    assert!(app.list_all_items(SUB, 10).await.is_empty());

    // Read-all while archived, then unarchive: comes back read (read state
    // never changed with archive state).
    let res = app.post_inbox(SUB, "/v1/inbox/read-all").await;
    assert_eq!(res.status(), 200);
    let res = app
        .post_inbox(SUB, &format!("/v1/inbox/broadcasts/{id}/unarchive"))
        .await;
    assert_eq!(res.status(), 204);
    let items = app.list_all_items(SUB, 10).await;
    assert_eq!(items.len(), 1);
    assert!(items[0]["read"].as_bool().unwrap(), "unarchived as read");
    assert!(!items[0]["archived"].as_bool().unwrap());
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 0);
}

#[tokio::test]
async fn archive_read_job_archives_read_items_across_chunk_boundaries() {
    let app = support::spawn().await;
    // 505 direct notifications cross the 500-per-chunk keyset boundary.
    let payload: Vec<serde_json::Value> = (0..505)
        .map(|_| json!({ "subscriber_id": SUB, "category": "bulk" }))
        .collect();
    for chunk in payload.chunks(100) {
        for body in chunk {
            let res = app
                .mgmt_post("/v1/notifications", body.clone())
                .send()
                .await
                .expect("create");
            assert_eq!(res.status(), 201);
        }
    }
    let created = app.create_broadcast("announce").await;
    let bcast = created["id"].as_str().unwrap().to_owned();

    // Everything read except two fresh arrivals after the read-all.
    let res = app.post_inbox(SUB, "/v1/inbox/read-all").await;
    assert_eq!(res.status(), 200);
    app.create_notification(SUB, "fresh").await;
    app.create_notification(SUB, "fresh").await;
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 2);

    let res = app.post_inbox(SUB, "/v1/inbox/archive-read").await;
    assert_eq!(res.status(), 202);
    app.drain_jobs().await;

    // Read items (505 direct + the broadcast) archived; the two unread
    // arrivals stay, counted exactly as before.
    let default_view = app.list_all_items(SUB, 100).await;
    assert_eq!(default_view.len(), 2, "only the unread arrivals remain");
    assert!(default_view.iter().all(|i| !i["read"].as_bool().unwrap()));
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 2, "the job never touched the counter");
    let archived_view = list_filtered(&app, SUB, "archived").await;
    assert_eq!(archived_view.len(), 506);
    assert!(
        archived_view.iter().any(|i| i["id"] == bcast.as_str()),
        "the read broadcast was archived too"
    );
}
