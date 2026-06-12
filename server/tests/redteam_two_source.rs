//! Regression guards for the two-source mute invariant (CLAUDE.md: "The list,
//! the unread count, and read state must agree across both sources at all
//! times").
//!
//! These pin the fix for a class of bug where a muted item left the list but
//! stayed in the unread count. Both count terms are now mute-aware:
//!   * the live BROADCAST term in `fetch_counts` evaluates the list arm
//!     exactly (visible, above the watermark, no read exception, not muted),
//!     so it agrees with the list at all times rather than relying on a
//!     `counter_rebuild` that can never reconcile a live, unstored term;
//!   * the maintained DIRECT counter is mute-aware on every path that writes
//!     it (immediate insert, scheduled deliver, individual read), so a
//!     notification entering an already-muted category is never counted, and
//!     marking a muted item read never steals a count from an unmuted one.
//!
//! Before the fix the proptest `Model` encoded the bug as expected (mute-blind
//! broadcast term; `rebuild` skipping broadcasts) and never asserted
//! count==visible-unread under mutes; that suite now does.

mod support;

use serde_json::json;

/// Mute (enabled=false) or unmute (enabled=true) one category for a subscriber
/// via the subscriber-plane preferences PUT. Returns once the 200 lands; the
/// PUT has enqueued a `counter_rebuild` job (the documented reconciliation).
async fn set_mute(app: &support::TestApp, subscriber: &str, category: &str, muted: bool) {
    let res = app
        .client
        .put(format!("{}/v1/inbox/preferences", app.base))
        .headers(app.subscriber_headers(subscriber))
        .json(&json!({ "preferences": [
            { "category": category, "channel": "in_app", "enabled": !muted }
        ]}))
        .send()
        .await
        .expect("set preference");
    assert_eq!(res.status(), 200, "set preference failed");
}

fn visible_unread(items: &[serde_json::Value]) -> i64 {
    items
        .iter()
        .filter(|i| !i["read"].as_bool().expect("read flag"))
        .count() as i64
}

/// A muted broadcast must leave the unread count, not just the list. Even
/// after a `counter_rebuild` runs, the live broadcast term stays mute-aware.
#[tokio::test]
async fn broadcast_mute_unread_count_agrees_with_list_after_rebuild() {
    let app = support::spawn().await;
    let sub = "usr_bcast_mute";

    // Subscriber exists before the broadcast, so the broadcast is visible to it
    // (visibility rule: broadcast.created_at >= subscriber.created_at).
    let res = app
        .client
        .put(format!("{}/v1/subscribers/{sub}", app.base))
        .bearer_auth(&app.env.api_key)
        .send()
        .await
        .expect("upsert subscriber");
    assert_eq!(res.status(), 200);

    app.create_broadcast("promo").await;
    app.drain_jobs().await;

    // Pre-mute: count and list agree (1 unread broadcast on both surfaces).
    let (unread_before, _) = app.counts(sub).await;
    let items_before = app.list_all_items(sub, 10).await;
    assert_eq!(unread_before, 1, "pre-mute unread count");
    assert_eq!(visible_unread(&items_before), 1, "pre-mute visible unread");

    // Mute the broadcast's category. The PUT enqueues counter_rebuild — the
    // schema's documented "eventual exactness" mechanism.
    set_mute(&app, sub, "promo", true).await;
    // Run it (and any hints). After this the reconciliation has completed.
    app.drain_jobs().await;
    assert_eq!(
        app.job_count(app.env.id).await,
        0,
        "counter_rebuild must have been processed, not just enqueued",
    );

    // The list now excludes the muted broadcast: zero visible items.
    let items_after = app.list_all_items(sub, 10).await;
    assert_eq!(items_after.len(), 0, "muted broadcast leaves the list");

    let (unread_after, _) = app.counts(sub).await;

    // The invariant: unread count must equal the number of unread items the
    // subscriber can actually see. The rebuild has run, yet they disagree.
    assert_eq!(
        unread_after,
        visible_unread(&items_after),
        "two-source invariant violated: /v1/inbox/counts reports unread={unread_after} \
         but the list shows {} unread item(s). A muted broadcast is counted forever; \
         counter_rebuild only reconciles the direct counters, never the live broadcast term.",
        visible_unread(&items_after),
    );
}

/// A direct notification created into an ALREADY-muted category must not be
/// counted: the list excludes it, so the count must too. (Parallel to the
/// broadcast hole: the mute-blind increment would otherwise count it forever,
/// since no preference flip follows to trigger a reconciliation.)
#[tokio::test]
async fn direct_notification_into_already_muted_category_is_not_counted() {
    let app = support::spawn().await;
    let sub = "usr_premute";

    // Mute "promo" BEFORE any promo notification exists, then settle the rebuild.
    set_mute(&app, sub, "promo", true).await;
    app.drain_jobs().await;

    // A direct notification now arrives in the muted category.
    app.create_notification(sub, "promo").await;
    app.drain_jobs().await;

    let items = app.list_all_items(sub, 10).await;
    assert_eq!(items.len(), 0, "muted direct notification leaves the list");

    let (unread, _) = app.counts(sub).await;
    assert_eq!(
        unread,
        visible_unread(&items),
        "direct notification into an already-muted category must not be counted: \
         count={unread}, visible unread items={}",
        visible_unread(&items),
    );
}

/// Marking an already-muted direct notification read must not under-count a
/// legitimately-unread item (mute-aware increment must pair with a mute-aware
/// decrement). With a mute-blind decrement, marking the muted item read
/// decrements the counter that the rebuild had already excluded it from,
/// stealing a count from the unmuted item. The `greatest(0, ...)` clamp hides
/// this only when the count is already zero, so a non-muted unread item is
/// present here to make the drift observable.
#[tokio::test]
async fn marking_a_muted_direct_notification_read_does_not_steal_a_count() {
    let app = support::spawn().await;
    let sub = "usr_mutedread";

    let muted = app.create_notification(sub, "promo").await; // will be muted
    let muted_id = muted["notifications"][0]["id"].as_str().unwrap().to_owned();
    app.create_notification(sub, "news").await; // stays visible + unread
    app.drain_jobs().await;

    set_mute(&app, sub, "promo", true).await; // rebuild recounts: only "news" is unread
    app.drain_jobs().await;

    let res = app
        .post_inbox(sub, &format!("/v1/inbox/notifications/{muted_id}/read"))
        .await;
    assert_eq!(res.status(), 204);

    let items = app.list_all_items(sub, 10).await;
    let (unread, _) = app.counts(sub).await;
    assert_eq!(
        unread,
        visible_unread(&items),
        "marking a muted item read must not steal a count from the unmuted unread item: \
         count={unread}, visible unread items={}",
        visible_unread(&items),
    );
}

/// CONTROL (passes): the identical flow with a DIRECT notification reconciles
/// correctly — counter_rebuild rewrites the direct counter mute-aware, so after
/// the drain the count matches the (empty) visible-unread list. This isolates
/// the defect to the broadcast term specifically.
#[tokio::test]
async fn direct_mute_unread_count_reconciles_with_list_after_rebuild() {
    let app = support::spawn().await;
    let sub = "usr_direct_mute";

    app.create_notification(sub, "promo").await;
    app.drain_jobs().await;

    let (unread_before, _) = app.counts(sub).await;
    let items_before = app.list_all_items(sub, 10).await;
    assert_eq!(unread_before, 1, "pre-mute unread count");
    assert_eq!(visible_unread(&items_before), 1, "pre-mute visible unread");

    set_mute(&app, sub, "promo", true).await;
    app.drain_jobs().await;
    assert_eq!(
        app.job_count(app.env.id).await,
        0,
        "counter_rebuild processed"
    );

    let items_after = app.list_all_items(sub, 10).await;
    assert_eq!(
        items_after.len(),
        0,
        "muted direct notification leaves the list"
    );

    let (unread_after, _) = app.counts(sub).await;
    assert_eq!(
        unread_after,
        visible_unread(&items_after),
        "direct mutes DO reconcile via counter_rebuild (count=0, list=0)",
    );
}
