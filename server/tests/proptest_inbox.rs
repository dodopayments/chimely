//! Proptest suites drive random interleavings of create, schedule, read,
//! read-all, seen-all, preference-flip, and broadcast against a real server
//! and Postgres, asserting the list, the counts, and read state stay in
//! agreement across both sources.
//!
//! One container per suite. Each case runs in a fresh environment. Ops are
//! separated by a 1ms pause so distinct transactions cannot share a
//! microsecond-identical now(). The watermark comparisons are strict about
//! ordering.

mod support;

use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Duration;

use proptest::prelude::*;
use serde_json::json;
use support::{TestApp, TestEnvironment};

const SUB: &str = "usr_prop";
const CATEGORIES: [&str; 3] = ["cat_a", "cat_b", "cat_c"];

#[derive(Clone, Debug)]
enum Op {
    CreateDirect {
        category: usize,
    },
    /// deliver_at one hour out. Durable but invisible and uncounted for the
    /// whole test. The short-horizon deliver flow is covered in worker.rs.
    CreateScheduledFar {
        category: usize,
    },
    CreateBroadcast {
        category: usize,
    },
    MarkDirectRead {
        pick: usize,
    },
    MarkDirectUnread {
        pick: usize,
    },
    MarkBroadcastRead {
        pick: usize,
    },
    MarkBroadcastUnread {
        pick: usize,
    },
    ReadAll,
    SeenAll,
    SetPreference {
        category: usize,
        enabled: bool,
    },
}

fn op_strategy(with_preferences: bool) -> impl Strategy<Value = Op> {
    let base = prop_oneof![
        (0..CATEGORIES.len()).prop_map(|category| Op::CreateDirect { category }),
        (0..CATEGORIES.len()).prop_map(|category| Op::CreateScheduledFar { category }),
        (0..CATEGORIES.len()).prop_map(|category| Op::CreateBroadcast { category }),
        any::<usize>().prop_map(|pick| Op::MarkDirectRead { pick }),
        any::<usize>().prop_map(|pick| Op::MarkDirectUnread { pick }),
        any::<usize>().prop_map(|pick| Op::MarkBroadcastRead { pick }),
        any::<usize>().prop_map(|pick| Op::MarkBroadcastUnread { pick }),
        Just(Op::ReadAll),
        Just(Op::SeenAll),
    ];
    if with_preferences {
        prop_oneof![
            base,
            ((0..CATEGORIES.len()), any::<bool>())
                .prop_map(|(category, enabled)| Op::SetPreference { category, enabled }),
        ]
        .boxed()
    } else {
        base.boxed()
    }
}

#[derive(Clone, Debug)]
struct ModelItem {
    /// Sequence position. Proxy for the DB ordering timestamp.
    order: usize,
    api_id: String,
    category: usize,
    broadcast: bool,
    /// Explicit per-item override. None means the watermark decides.
    explicit_read: Option<bool>,
}

#[derive(Default)]
struct Model {
    items: Vec<ModelItem>,
    /// Ops with order <= these are covered by the corresponding watermark.
    read_watermark: Option<usize>,
    seen_watermark: Option<usize>,
    /// Maintained direct counters. Mute-blind increments, mute-aware recount
    /// on preference flips.
    unread_direct: i64,
    unseen_direct: i64,
    muted: HashSet<usize>,
}

impl Model {
    fn read(&self, item: &ModelItem) -> bool {
        item.explicit_read
            .unwrap_or_else(|| self.read_watermark.is_some_and(|w| item.order <= w))
    }

    fn watermark_covers(&self, item: &ModelItem) -> bool {
        self.read_watermark.is_some_and(|w| item.order <= w)
    }

    fn seen(&self, item: &ModelItem) -> bool {
        self.seen_watermark.is_some_and(|w| item.order <= w)
    }

    fn visible_unmuted(&self) -> Vec<&ModelItem> {
        let mut v: Vec<&ModelItem> = self
            .items
            .iter()
            .filter(|i| !self.muted.contains(&i.category))
            .collect();
        v.sort_by_key(|i| std::cmp::Reverse(i.order));
        v
    }

    fn expected_unread(&self) -> i64 {
        // The broadcast terms are evaluated exactly as the list arm: explicit
        // override outranks the watermark, so the count agrees with the
        // visible list at all times.
        let broadcast_unread = self
            .items
            .iter()
            .filter(|i| i.broadcast && !self.muted.contains(&i.category) && !self.read(i))
            .count() as i64;
        self.unread_direct + broadcast_unread
    }

    fn expected_unseen(&self) -> i64 {
        self.unseen_direct
            + self
                .items
                .iter()
                .filter(|i| {
                    i.broadcast
                        && !self.muted.contains(&i.category)
                        && self.seen_watermark.is_none_or(|w| i.order > w)
                })
                .count() as i64
    }

    /// The mute-aware recount the counter_rebuild job performs.
    fn rebuild(&mut self) {
        self.unread_direct = self
            .items
            .iter()
            .filter(|i| !i.broadcast && !self.muted.contains(&i.category) && !self.read(i))
            .count() as i64;
        self.unseen_direct = self
            .items
            .iter()
            .filter(|i| !i.broadcast && !self.muted.contains(&i.category) && !self.seen(i))
            .count() as i64;
    }
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Runtime::new().expect("test runtime"))
}

fn app() -> &'static TestApp {
    static APP: OnceLock<TestApp> = OnceLock::new();
    APP.get_or_init(|| runtime().block_on(support::spawn()))
}

async fn run_case(app: &TestApp, ops: Vec<Op>, check_list_equality: bool) {
    let env = app.create_environment(true).await;

    // Subscriber exists before any broadcast, so every broadcast is visible.
    let res = app
        .client
        .put(format!("{}/v1/subscribers/{SUB}", app.base))
        .bearer_auth(&env.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    let mut model = Model::default();
    for (order, op) in ops.into_iter().enumerate() {
        tokio::time::sleep(Duration::from_millis(1)).await;
        apply_op(app, &env, &mut model, order, op).await;
    }
    drain(app).await;

    // ---- list ↔ model agreement (read state across both sources) ----------
    let items = list_all(app, &env).await;
    let expected = model.visible_unmuted();
    assert_eq!(items.len(), expected.len(), "list length\n{items:#?}");
    for (got, want) in items.iter().zip(&expected) {
        assert_eq!(got["id"].as_str().unwrap(), want.api_id, "merged order");
        assert_eq!(got["category"].as_str().unwrap(), CATEGORIES[want.category]);
        assert_eq!(
            got["read"].as_bool().unwrap(),
            model.read(want),
            "read state for {} (order {})",
            want.api_id,
            want.order,
        );
    }

    // ---- unread view ↔ model agreement -------------------------------------
    // The filtered list must be exactly the unread projection of the default
    // list, in the same order, across both sources.
    let unread_items = list_all_filtered(app, &env, "unread").await;
    let expected_unread_ids: Vec<&str> = expected
        .iter()
        .filter(|i| !model.read(i))
        .map(|i| i.api_id.as_str())
        .collect();
    let unread_ids: Vec<&str> = unread_items
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert_eq!(unread_ids, expected_unread_ids, "unread view");
    assert!(
        unread_items.iter().all(|i| !i["read"].as_bool().unwrap()),
        "unread view returned a read item"
    );

    // ---- counts ↔ model agreement ------------------------------------------
    let (unread, unseen) = counts(app, &env).await;
    assert_eq!(unread, model.expected_unread(), "unread count");
    assert_eq!(unseen, model.expected_unseen(), "unseen count");

    // count == visible unread items, exact at all times including under
    // mutes. Every counter path (insert, deliver, individual read, broadcast
    // term) is mute-aware, and watermark moves rebuild before the assertion
    // drains. A muted item drops from both the list and the count together.
    if check_list_equality {
        let visible_unread = items
            .iter()
            .filter(|i| !i["read"].as_bool().unwrap())
            .count() as i64;
        assert_eq!(
            unread, visible_unread,
            "unread count vs visible unread items"
        );
        let visible_unseen = expected.iter().filter(|i| !model.seen(i)).count() as i64;
        assert_eq!(
            unseen, visible_unseen,
            "unseen count vs visible unseen items"
        );
    }
}

async fn apply_op(app: &TestApp, env: &TestEnvironment, model: &mut Model, order: usize, op: Op) {
    match op {
        Op::CreateDirect { category } => {
            let res = mgmt(
                app,
                env,
                "/v1/notifications",
                json!({ "subscriber_id": SUB, "category": CATEGORIES[category] }),
            )
            .await;
            let id = res["notifications"][0]["id"].as_str().unwrap().to_owned();
            model.items.push(ModelItem {
                order,
                api_id: id,
                category,
                broadcast: false,
                explicit_read: None,
            });
            // A create into an already-muted category is not counted,
            // matching the list arm.
            if !model.muted.contains(&category) {
                model.unread_direct += 1;
                model.unseen_direct += 1;
            }
        }
        Op::CreateScheduledFar { category } => {
            let deliver_at = chrono::Utc::now() + chrono::Duration::hours(1);
            mgmt(
                app,
                env,
                "/v1/notifications",
                json!({ "subscriber_id": SUB, "category": CATEGORIES[category],
                        "deliver_at": deliver_at.to_rfc3339() }),
            )
            .await;
        }
        Op::CreateBroadcast { category } => {
            let res = mgmt(
                app,
                env,
                "/v1/broadcasts",
                json!({ "category": CATEGORIES[category] }),
            )
            .await;
            let id = res["id"].as_str().unwrap().to_owned();
            model.items.push(ModelItem {
                order,
                api_id: id,
                category,
                broadcast: true,
                explicit_read: None,
            });
        }
        Op::MarkDirectRead { pick } => {
            // Any direct item, muted or not. The mute-aware decrement makes
            // marking a muted item read a counter no-op since it was never
            // counted.
            let candidates: Vec<usize> = model
                .items
                .iter()
                .enumerate()
                .filter(|(_, i)| !i.broadcast)
                .map(|(idx, _)| idx)
                .collect();
            if candidates.is_empty() {
                return;
            }
            let idx = candidates[pick % candidates.len()];
            let id = model.items[idx].api_id.clone();
            let res = inbox_post(app, env, &format!("/v1/inbox/notifications/{id}/read")).await;
            assert_eq!(res, 204);
            // A muted row was never counted, so marking it read must not
            // decrement, else it steals a count from an unmuted item.
            let muted = model.muted.contains(&model.items[idx].category);
            if !muted && !model.read(&model.items[idx]) {
                model.unread_direct -= 1;
            }
            model.items[idx].explicit_read = Some(true);
        }
        Op::MarkDirectUnread { pick } => {
            let candidates: Vec<usize> = model
                .items
                .iter()
                .enumerate()
                .filter(|(_, i)| !i.broadcast)
                .map(|(idx, _)| idx)
                .collect();
            if candidates.is_empty() {
                return;
            }
            let idx = candidates[pick % candidates.len()];
            let id = model.items[idx].api_id.clone();
            let res = inbox_post(app, env, &format!("/v1/inbox/notifications/{id}/unread")).await;
            assert_eq!(res, 204);
            // Server no-op on an already-unread item. On a read item the
            // increment mirrors the mark-read decrement's mute guard.
            if model.read(&model.items[idx]) {
                if !model.muted.contains(&model.items[idx].category) {
                    model.unread_direct += 1;
                }
                // Above the watermark, clearing read_at alone means unread
                // (no override row). At or below, an override survives it.
                model.items[idx].explicit_read = if model.watermark_covers(&model.items[idx]) {
                    Some(false)
                } else {
                    None
                };
            }
        }
        Op::MarkBroadcastRead { pick } => {
            let candidates: Vec<usize> = model
                .items
                .iter()
                .enumerate()
                .filter(|(_, i)| i.broadcast)
                .map(|(idx, _)| idx)
                .collect();
            if candidates.is_empty() {
                return;
            }
            let idx = candidates[pick % candidates.len()];
            let id = model.items[idx].api_id.clone();
            let res = inbox_post(app, env, &format!("/v1/inbox/broadcasts/{id}/read")).await;
            assert_eq!(res, 204);
            // Above the watermark an explicit read row lands. At or below,
            // the server only deletes a possible unread override and the
            // watermark reads the item.
            model.items[idx].explicit_read = if model.watermark_covers(&model.items[idx]) {
                None
            } else {
                Some(true)
            };
        }
        Op::MarkBroadcastUnread { pick } => {
            let candidates: Vec<usize> = model
                .items
                .iter()
                .enumerate()
                .filter(|(_, i)| i.broadcast)
                .map(|(idx, _)| idx)
                .collect();
            if candidates.is_empty() {
                return;
            }
            let idx = candidates[pick % candidates.len()];
            let id = model.items[idx].api_id.clone();
            let res = inbox_post(app, env, &format!("/v1/inbox/broadcasts/{id}/unread")).await;
            assert_eq!(res, 204);
            // At or below the watermark an unread override lands. Above it,
            // deleting the read row is the whole operation.
            if model.watermark_covers(&model.items[idx]) {
                model.items[idx].explicit_read = Some(false);
            } else if model.items[idx].explicit_read == Some(true) {
                model.items[idx].explicit_read = None;
            }
        }
        Op::ReadAll => {
            let res = inbox_post(app, env, "/v1/inbox/read-all").await;
            assert_eq!(res, 200);
            model.read_watermark = Some(order);
            model.unread_direct = 0;
            // Overrides of both polarities die: broadcast rows GC'd, direct
            // unread_at cleared. Every existing item sits below the new
            // watermark.
            for item in &mut model.items {
                item.explicit_read = None;
            }
        }
        Op::SeenAll => {
            let res = inbox_post(app, env, "/v1/inbox/seen-all").await;
            assert_eq!(res, 200);
            model.seen_watermark = Some(order);
            model.unseen_direct = 0;
        }
        Op::SetPreference { category, enabled } => {
            let res = app
                .client
                .put(format!("{}/v1/inbox/preferences", app.base))
                .headers(app.subscriber_headers_for(env, SUB))
                .json(&json!({ "preferences": [{
                    "category": CATEGORIES[category], "channel": "in_app", "enabled": enabled,
                }]}))
                .send()
                .await
                .unwrap();
            assert_eq!(res.status(), 200);
            let changed = if enabled {
                model.muted.remove(&category)
            } else {
                model.muted.insert(category)
            };
            if changed {
                // The PUT enqueued counter_rebuild. Run it at this sequence
                // position so the model's recount matches the real one.
                drain(app).await;
                model.rebuild();
            }
        }
    }
}

// ----- thin HTTP helpers scoped to a per-case environment -------------------

async fn mgmt(
    app: &TestApp,
    env: &TestEnvironment,
    path: &str,
    body: serde_json::Value,
) -> serde_json::Value {
    let res = app
        .client
        .post(format!("{}{path}", app.base))
        .bearer_auth(&env.api_key)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201, "create failed");
    res.json().await.unwrap()
}

async fn inbox_post(app: &TestApp, env: &TestEnvironment, path: &str) -> u16 {
    app.client
        .post(format!("{}{path}", app.base))
        .headers(app.subscriber_headers_for(env, SUB))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

async fn counts(app: &TestApp, env: &TestEnvironment) -> (i64, i64) {
    let res = app
        .client
        .get(format!("{}/v1/inbox/counts", app.base))
        .headers(app.subscriber_headers_for(env, SUB))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = res.json().await.unwrap();
    (
        body["unread"].as_i64().unwrap(),
        body["unseen"].as_i64().unwrap(),
    )
}

async fn list_all(app: &TestApp, env: &TestEnvironment) -> Vec<serde_json::Value> {
    list_all_filtered(app, env, "").await
}

async fn list_all_filtered(
    app: &TestApp,
    env: &TestEnvironment,
    filter: &str,
) -> Vec<serde_json::Value> {
    let mut items = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut url = format!("{}/v1/inbox/items?limit=3", app.base); // tiny pages exercise the keyset
        if !filter.is_empty() {
            url.push_str(&format!("&filter={filter}"));
        }
        if let Some(c) = &cursor {
            url.push_str(&format!("&cursor={c}"));
        }
        let res = app
            .client
            .get(url)
            .headers(app.subscriber_headers_for(env, SUB))
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

async fn drain(app: &TestApp) {
    for _ in 0..200 {
        let due: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE run_at <= now() + interval '2 seconds'",
        )
        .fetch_one(&app.pool)
        .await
        .unwrap();
        if due == 0 {
            return;
        }
        if app.sweep().await == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    panic!("jobs never drained");
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 24,
        max_shrink_iters: 64,
        .. ProptestConfig::default()
    })]

    /// No preference ops. The unread count always equals the number of unread
    /// items visible in the list.
    #[test]
    fn unread_count_always_equals_visible_unread_items(
        ops in proptest::collection::vec(op_strategy(false), 1..14)
    ) {
        let app = app(); // initialized outside the runtime, else block_on nests
        runtime().block_on(run_case(app, ops, true));
    }

    /// With preference flips, the unread count still always equals the number
    /// of unread items visible in the list. Every counter path is mute-aware,
    /// so a muted item leaves the list and the count together.
    #[test]
    fn two_source_state_agrees_under_preference_flips(
        ops in proptest::collection::vec(op_strategy(true), 1..14)
    ) {
        let app = app(); // initialized outside the runtime, else block_on nests
        runtime().block_on(run_case(app, ops, true));
    }
}
