//! Task 4 acceptance: proptest suites driving random interleavings of
//! create / schedule / read / read-all / seen-all / preference-flip /
//! broadcast against a real server + Postgres, asserting that the list, the
//! counts, and read state stay in agreement across both sources (the
//! two-source merge and watermark invariants).
//!
//! One container per suite; each case runs in a fresh environment. Ops are
//! separated by a 1ms pause so distinct transactions cannot share a
//! microsecond-identical now() (the watermark comparisons are intentionally
//! strict about ordering).

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
    /// deliver_at one hour out: durable, but invisible and uncounted for the
    /// whole test (the short-horizon deliver flow is covered in worker.rs).
    CreateScheduledFar {
        category: usize,
    },
    CreateBroadcast {
        category: usize,
    },
    MarkDirectRead {
        pick: usize,
    },
    MarkBroadcastRead {
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
        any::<usize>().prop_map(|pick| Op::MarkBroadcastRead { pick }),
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
    /// Sequence position; the proxy for the DB ordering timestamp.
    order: usize,
    api_id: String,
    category: usize,
    broadcast: bool,
    read_exception: bool,
}

#[derive(Default)]
struct Model {
    items: Vec<ModelItem>,
    /// Ops with order <= these are covered by the corresponding watermark.
    read_watermark: Option<usize>,
    seen_watermark: Option<usize>,
    /// The maintained direct counters, tracked by the documented rules
    /// (mute-blind increments; mute-aware recount on preference flips).
    unread_direct: i64,
    unseen_direct: i64,
    muted: HashSet<usize>,
}

impl Model {
    fn read(&self, item: &ModelItem) -> bool {
        item.read_exception || self.read_watermark.is_some_and(|w| item.order <= w)
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
        let broadcast_above_watermark = self
            .items
            .iter()
            .filter(|i| i.broadcast && self.read_watermark.is_none_or(|w| i.order > w));
        // The canonical count: maintained direct counter + broadcast window
        // term − exception rows above the watermark. Mute-blind by design.
        self.unread_direct + broadcast_above_watermark.clone().count() as i64
            - broadcast_above_watermark
                .filter(|i| i.read_exception)
                .count() as i64
    }

    fn expected_unseen(&self) -> i64 {
        self.unseen_direct
            + self
                .items
                .iter()
                .filter(|i| i.broadcast && self.seen_watermark.is_none_or(|w| i.order > w))
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

    // Subscriber exists before any broadcast — every broadcast is visible.
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

    // ---- counts ↔ model agreement ------------------------------------------
    let (unread, unseen) = counts(app, &env).await;
    assert_eq!(unread, model.expected_unread(), "unread count");
    assert_eq!(unseen, model.expected_unseen(), "unseen count");

    // ---- the user-facing invariant: count == visible unread items ----------
    // Exact whenever no category is muted (counters are mute-blind by
    // documented design, so a muted unread item drops from the list while
    // remaining counted until the next rebuild).
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
                read_exception: false,
            });
            // Mute-blind conditional increment (watermark is always behind us).
            model.unread_direct += 1;
            model.unseen_direct += 1;
        }
        Op::CreateScheduledFar { category } => {
            // Durable but invisible + uncounted for the whole test.
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
                read_exception: false,
            });
        }
        Op::MarkDirectRead { pick } => {
            // Only unmuted targets: marking a muted item read is documented
            // counter drift (mute-blind decrement vs mute-aware rebuild)
            // healed by the next rebuild — not an invariant violation to test.
            let candidates: Vec<usize> = model
                .items
                .iter()
                .enumerate()
                .filter(|(_, i)| !i.broadcast && !model.muted.contains(&i.category))
                .map(|(idx, _)| idx)
                .collect();
            if candidates.is_empty() {
                return;
            }
            let idx = candidates[pick % candidates.len()];
            let id = model.items[idx].api_id.clone();
            let res = inbox_post(app, env, &format!("/v1/inbox/notifications/{id}/read")).await;
            assert_eq!(res, 204);
            if !model.read(&model.items[idx]) {
                model.unread_direct -= 1;
            }
            model.items[idx].read_exception = true;
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
            // Below the watermark this is a server-side no-op; the model's
            // read() already reports it read either way.
            model.items[idx].read_exception = true;
        }
        Op::ReadAll => {
            let res = inbox_post(app, env, "/v1/inbox/read-all").await;
            assert_eq!(res, 200);
            model.read_watermark = Some(order);
            model.unread_direct = 0;
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
                // The PUT enqueued counter_rebuild; run it AT this sequence
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
    let mut items = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut url = format!("{}/v1/inbox/items?limit=3", app.base); // tiny pages: exercise the keyset
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

    /// No preference ops ⇒ the strongest form of the invariant: the unread
    /// count ALWAYS equals the number of unread items visible in the list.
    #[test]
    fn unread_count_always_equals_visible_unread_items(
        ops in proptest::collection::vec(op_strategy(false), 1..14)
    ) {
        let app = app(); // initialized outside the runtime: block_on nests otherwise
        runtime().block_on(run_case(app, ops, true));
    }

    /// With preference flips: list, counts, and read state must agree with
    /// the documented counter semantics (mute-blind increments, mute-aware
    /// rebuild at the flip).
    #[test]
    fn two_source_state_agrees_under_preference_flips(
        ops in proptest::collection::vec(op_strategy(true), 1..14)
    ) {
        let app = app(); // initialized outside the runtime: block_on nests otherwise
        runtime().block_on(run_case(app, ops, false));
    }
}
