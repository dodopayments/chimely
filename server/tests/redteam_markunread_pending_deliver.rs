//! Guards the single-bookkeeper rule for rows owned by a pending deliver job
//! across a read -> unread flip.
//!
//! A visible row still owned by a pending deliver job is uncounted: the
//! deliver bump is its only counter. mark-read skips its decrement for such
//! rows, and mark-unread must symmetrically skip its increment, or the
//! deliver bump (whose condition sees the row as unread again) would count it
//! a second time and drift the counter +1 forever.

mod support;

use std::time::Duration;

use chrono::Utc;
use serde_json::json;

const SUB: &str = "usr_unread_pending";

#[tokio::test]
async fn read_then_unread_on_a_pending_deliver_row_counts_once() {
    let app = support::spawn().await;

    let res = app
        .client
        .put(format!("{}/v1/subscribers/{SUB}", app.base))
        .bearer_auth(&app.env.api_key)
        .send()
        .await
        .expect("upsert subscriber");
    assert_eq!(res.status(), 200);

    // Scheduled notification with a near deliver_at. Durable immediately,
    // owned by the deliver job until a sweep processes it.
    let deliver_at = Utc::now() + chrono::Duration::milliseconds(300);
    let created = app
        .mgmt_post(
            "/v1/notifications",
            json!({ "subscriber_id": SUB, "category": "reminder",
                    "deliver_at": deliver_at.to_rfc3339() }),
        )
        .send()
        .await
        .expect("create scheduled");
    assert_eq!(created.status(), 201);
    let body: serde_json::Value = created.json().await.expect("create body");
    let notif_id = body["notifications"][0]["id"].as_str().unwrap().to_owned();

    // Wait past deliver_at WITHOUT sweeping: the row is visible but still
    // owned by the pending job, so it is uncounted.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 0, "pending row is uncounted before the sweep");

    // Read then unread through the real handlers, both while the job is
    // pending. Neither may touch the counter: mark-read skips the decrement
    // (pending), mark-unread skips the increment (pending). The row ends
    // read_at NULL again, so the deliver bump will count it.
    let res = app
        .client
        .post(format!(
            "{}/v1/inbox/notifications/{notif_id}/read",
            app.base
        ))
        .headers(app.subscriber_headers(SUB))
        .send()
        .await
        .expect("mark read");
    assert_eq!(res.status(), 204);
    let res = app
        .client
        .post(format!(
            "{}/v1/inbox/notifications/{notif_id}/unread",
            app.base
        ))
        .headers(app.subscriber_headers(SUB))
        .send()
        .await
        .expect("mark unread");
    assert_eq!(res.status(), 204);
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 0, "neither flip touched the counter while pending");

    // The deliver bump is the single bookkeeper: exactly one +1.
    while app.sweep().await == 0 {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let items = app.list_all_items(SUB, 10).await;
    let visible_unread = items
        .iter()
        .filter(|i| !i["read"].as_bool().unwrap())
        .count() as i64;
    assert_eq!(visible_unread, 1, "the item is unread in the list");
    let (unread, _) = app.counts(SUB).await;
    assert_eq!(unread, 1, "counted exactly once, by the deliver bump");
    assert_eq!(unread, visible_unread, "two-source invariant holds");
}
