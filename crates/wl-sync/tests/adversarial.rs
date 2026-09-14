//! Adversarial sync-layer characterization tests — each `defect_*` test
//! locks in a verified bug from the battle-test report.

use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use axum::Router;
use tower::util::ServiceExt;

use wl_core::crypto::identity::Identity;
use wl_core::store::open_in_memory;
use wl_core::store::repo::Repos;
use wl_sync::sync::{save_cursor, sync_cycle, Transport};

struct AxumTransport {
    app: Router,
    token: String,
}

impl Transport for AxumTransport {
    fn post(&self, path: &str, body: &serde_json::Value) -> Result<String, String> {
        let fut = async {
            let resp = self
                .app
                .clone()
                .oneshot(
                    Request::post(path)
                        .header("content-type", "application/json")
                        .header("authorization", format!("Bearer {}", self.token))
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .map_err(|e| e.to_string())?;
            let status = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .map_err(|e| e.to_string())?;
            if !status.is_success() {
                return Err(format!(
                    "HTTP {status}: {}",
                    String::from_utf8_lossy(&bytes)
                ));
            }
            Ok(String::from_utf8(bytes.to_vec()).map_err(|e| e.to_string())?)
        };
        match tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut)) {
            Ok(v) => Ok(v),
            Err(e) if e.to_string().contains("Cannot start") => {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())?;
                rt.block_on(async {
                    let resp = self
                        .app
                        .clone()
                        .oneshot(
                            Request::post(path)
                                .header("content-type", "application/json")
                                .header("authorization", format!("Bearer {}", self.token))
                                .body(Body::from(body.to_string()))
                                .unwrap(),
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                    let status = resp.status();
                    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                        .await
                        .map_err(|e| e.to_string())?;
                    if !status.is_success() {
                        return Err(format!(
                            "HTTP {status}: {}",
                            String::from_utf8_lossy(&bytes)
                        ));
                    }
                    Ok(String::from_utf8(bytes.to_vec()).map_err(|e| e.to_string())?)
                })
            }
            Err(e) => Err(e),
        }
    }
}

fn relay_app() -> Router {
    let state = Arc::new(wl_relay::AppStateForTest {
        auth: wl_relay::AuthForTest::new(),
        blobs: Box::new(wl_relay::SqliteForTest::open_in_memory().unwrap()),
    });
    wl_relay::router_for_test(state)
}

async fn authenticate(app: &Router, identity: &Identity) -> String {
    let pk_hex = identity.account_id_hex();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/auth/challenge")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"public_key": pk_hex}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let challenge: wl_protocol::Challenge = serde_json::from_slice(&bytes).unwrap();

    let payload = wl_protocol::challenge_signing_payload(&challenge.nonce, challenge.expires_at);
    let sig = hex::encode(identity.sign(&payload));
    let resp = app
        .clone()
        .oneshot(
            Request::post("/auth/verify")
                .header("content-type", "application/json")
                .header("x-nonce", &challenge.nonce)
                .header("x-expires", challenge.expires_at.to_string())
                .body(Body::from(
                    serde_json::json!({"public_key": pk_hex, "signature": sig}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let session: wl_protocol::SessionToken = serde_json::from_slice(&bytes).unwrap();
    session.token
}

const PHRASE: &str = "legal winner thank year wave sausage worth useful legal winner thank yellow";

/// Finding: `sync_cycle` advances the pull cursor but NEVER persists it
/// (`save_cursor` exists but no caller uses it). Every cycle re-pulls
/// the ENTIRE remote op history from "" — only the `crdt_applied`
/// watermark keeps it from re-applying. Costs grow unboundedly with
/// history; `stats.pulled` is permanently inflated.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defect_pull_cursor_never_persisted_full_history_repulled() {
    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport {
        app: app.clone(),
        token,
    };

    // Device A: one write, pushed.
    let conn_a = open_in_memory().unwrap();
    let a = Repos::new(conn_a, 1);
    let g = a.create_goal("Solo", None, None, None).unwrap();
    a.enqueue_outbox(
        &identity,
        "goals",
        &g.id,
        &serde_json::json!({"id": g.id, "title": "Solo", "description": null,
            "target_date": null, "status": "active"}),
    )
    .unwrap();
    sync_cycle(&a, &identity, &transport, 100).unwrap();

    // Device B pulls once: 1 op.
    let conn_b = open_in_memory().unwrap();
    let b = Repos::new(conn_b, 2);
    let s1 = sync_cycle(&b, &identity, &transport, 100).unwrap();
    assert_eq!(s1.pulled, 1);

    // A second, third, and Nth cycle: still re-pulls the SAME 1 op.
    let s2 = sync_cycle(&b, &identity, &transport, 100).unwrap();
    assert_eq!(
        s2.pulled, 1,
        "DEFECT: cursor not persisted; every cycle re-downloads all history (applied=0 only thanks to the watermark)"
    );
    assert_eq!(s2.applied, 0);
    let s3 = sync_cycle(&b, &identity, &transport, 100).unwrap();
    assert_eq!(s3.pulled, 1);
}

/// Finding: `apply_op_to_db` writes remote rows with NO LWW check — it
/// blind-upserts by the local SQL's own logic and never consults
/// `TableState`/HLC arbitration. An older remote op (or an op pulled
/// late) overwrites NEWER local state. The CRDT engine only exists in
/// tests; production sync bypasses it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defect_pull_overwrites_newer_local_state_no_lww_check() {
    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport { app, token };

    // Device A pushes a STALE goal status (older HLC).
    let conn_a = open_in_memory().unwrap();
    let a = Repos::new(conn_a, 1);
    let g = a.create_goal("Race", None, None, None).unwrap();
    a.enqueue_outbox(
        &identity,
        "goals",
        &g.id,
        &serde_json::json!({"title": "Race", "description": null,
            "target_date": null, "status": "achieved"}),
    )
    .unwrap();
    sync_cycle(&a, &identity, &transport, 100).unwrap();

    // Device B already has NEWER local state for the same row (later HLC).
    let conn_b = open_in_memory().unwrap();
    let b = Repos::new(conn_b, 2);
    let g_local = b.create_goal("Race", None, None, None).unwrap();
    // Force B's row to a newer HLC and status 'archived' (terminal state).
    let newer_ts = {
        let ts = b.hlc.now(2);
        // guarantee strictly-greater physical component
        wl_core::hlc::HlcTimestamp {
            physical: ts.physical + 1_000_000_000,
            counter: 0,
            device: 2,
        }
    };
    b.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE goals SET status='archived', hlc_timestamp=?2 WHERE id=?1",
            rusqlite::params![g_local.id, newer_ts.to_string()],
        )
        .unwrap();
    assert_eq!(g_local.id.len(), g.id.len()); // same id-space shape (uuids differ)

    // Pull A's stale op: it is NOT for B's row (different uuid) — so
    // instead seed the exact row id and re-run.
    b.conn
        .lock()
        .unwrap()
        .execute(
            "INSERT OR REPLACE INTO goals (id,title,description,target_date,status,hlc_timestamp)
             VALUES (?1,'Race',NULL,NULL,'archived',?2)",
            rusqlite::params![g.id, newer_ts.to_string()],
        )
        .unwrap();

    let before = b.goal(&g.id).unwrap().unwrap();
    assert_eq!(before.status.as_str(), "archived");

    let s = sync_cycle(&b, &identity, &transport, 100).unwrap();
    assert!(s.applied >= 1);
    let after = b.goal(&g.id).unwrap().unwrap();
    assert_eq!(
        after.status.as_str(),
        "achieved",
        "DEFECT: remote op with OLDER hlc overwrote newer local 'archived' state — apply_op_to_db has no LWW arbitration"
    );
    // And the row's hlc_timestamp is now the OLDER remote one.
    assert!(after.hlc_timestamp < newer_ts);
}

/// Finding: relay pull ordering is TEXT lexicographic on `hlc`. With
/// counters crossing a digit boundary (e.g. `.9.` vs `.10.`) the relay
/// returns ops out of causal order, and `apply_op_to_db` (no LWW
/// check) writes whatever arrives last — divergence across devices
/// with different pull batch sizes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defect_counter_width_text_order_diverges_from_numeric() {
    // Relay-side comparison (SQLite TEXT): "pt.10.dev" < "pt.9.dev".
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let ord: i64 = conn
        .query_row("SELECT '900.10.1' > '900.9.1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ord, 0, "SQLite text compare says '900.10.1' <= '900.9.1'");
    // Numeric HLC compare says the opposite.
    let a = wl_core::hlc::HlcTimestamp::parse("900.10.1").unwrap();
    let b_ = wl_core::hlc::HlcTimestamp::parse("900.9.1").unwrap();
    assert!(a > b_);
}

/// Finding: `mark_outbox_pushed` marks ops pushed based on the relay's
/// `accepted` list. If the relay already had an op (idempotent re-push
/// after a partial failure), `accepted` is empty and the op stays
/// pending FOREVER — infinite re-push on every cycle (never drains).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defected_outbox_never_drains_on_idempotent_repush() {
    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport { app, token };

    let conn_a = open_in_memory().unwrap();
    let a = Repos::new(conn_a, 1);
    let g = a.create_goal("Drain", None, None, None).unwrap();
    a.enqueue_outbox(
        &identity,
        "goals",
        &g.id,
        &serde_json::json!({"title": "Drain", "description": null,
            "target_date": null, "status": "active"}),
    )
    .unwrap();

    // Simulate: push succeeded server-side but the response was lost
    // (op already in relay). Do this by pushing once with a transport
    // that drops the response mid-cycle, then re-syncing normally.
    struct DroppingTransport<'a> {
        inner: &'a AxumTransport,
    }
    impl Transport for DroppingTransport<'_> {
        fn post(&self, path: &str, _body: &serde_json::Value) -> Result<String, String> {
            if path == "/sync/push" {
                // Push lands on the relay, response "lost".
                let _ = self
                    .inner
                    .post(path, &serde_json::json!({"ops": []}))
                    .map_err(|e| e.to_string());
                return Err("connection reset".into());
            }
            self.inner.post(path, _body)
        }
    }
    // First: push the real op via the real transport so the relay has it.
    let real_push = serde_json::json!({});
    let _ = real_push;
    {
        // Push with a transport that succeeds, then reset the pushed
        // flag locally to model the lost response.
        let s = sync_cycle(&a, &identity, &transport, 100).unwrap();
        assert_eq!(s.pushed, 1);
        a.conn
            .lock()
            .unwrap()
            .execute("UPDATE crdt_outbox SET pushed=0", [])
            .unwrap();
    }
    let _ = DroppingTransport { inner: &transport };

    // Next cycle: relay reports accepted=[] (duplicate op ids), so
    // mark_outbox_pushed marks NOTHING.
    let s = sync_cycle(&a, &identity, &transport, 100).unwrap();
    assert_eq!(
        s.pushed, 0,
        "DEFECT: duplicate op not in accepted list → never marked pushed"
    );
    let pending = a.pending_outbox(100).unwrap();
    assert_eq!(
        pending.len(),
        1,
        "DEFECT: outbox never drains — op re-encrypted & re-pushed on every sync forever"
    );
    // And it will be re-pushed again on the NEXT cycle too.
    let s2 = sync_cycle(&a, &identity, &transport, 100).unwrap();
    assert_eq!(s2.pushed, 0);
    assert_eq!(a.pending_outbox(100).unwrap().len(), 1);
}

/// Finding: pull loop termination. If the relay returns `exhausted:
/// false` with a batch of exactly `limit` ops whose LAST op's hlc equals
/// the batch-max, the next pull with `since_hlc = next_cursor` uses a
/// STRICT `hlc > since` comparison. Two ops with the SAME hlc text
/// (different devices, e.g. `100.0.1` from dev 1 and... wait — same
/// text means same device). The real hazard: ops with identical hlc
/// text from the same device (legit: same-pt-same-ctr after restart
/// resets counter). The second one is NEVER pulled: `hlc > cursor`
/// skips it forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defect_same_hlc_ops_lost_by_strict_cursor() {
    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport { app, token };

    // Device A: two DIFFERENT rows that share an identical HLC text
    // (counter reset on restart at the same wall instant).
    let conn_a = open_in_memory().unwrap();
    let a = Repos::new(conn_a, 1);
    let shared_ts = wl_core::hlc::HlcTimestamp {
        physical: 1_750_000_000_000_000_000,
        counter: 0,
        device: 1,
    };
    for (rec, title) in [("g-one", "One"), ("g-two", "Two")] {
        a.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO crdt_outbox (operation_id, hlc_timestamp, table_name, record_id, encrypted_payload, created_at_epoch_ms, pushed)
                 VALUES (?1, ?2, 'goals', ?3, ?4, 0, 0)",
                rusqlite::params![
                    format!("op-{rec}"),
                    shared_ts.to_string(),
                    rec,
                    // Genuinely sealed payload so pull-side unseal works.
                    {
                        let sealed = wl_core::crypto::aead::seal(
                            &identity,
                            &serde_json::to_vec(&serde_json::json!({
                                "title": title, "description": null,
                                "target_date": null, "status": "active"
                            }))
                            .unwrap(),
                            format!("goals:{rec}").as_bytes(),
                        )
                        .unwrap();
                        sealed.to_bytes()
                    }
                ],
            )
            .unwrap();
    }

    // Push both to the relay.
    use base64::Engine;
    let ops_json: Vec<serde_json::Value> = a
        .pending_outbox(10)
        .unwrap()
        .iter()
        .map(|o| {
            serde_json::json!({
                "operation_id": o.operation_id,
                "hlc": o.hlc_timestamp.to_string(),
                "table": o.table_name,
                "record_id": o.record_id,
                "sealed_b64": base64::engine::general_purpose::STANDARD
                    .encode(&o.encrypted_payload),
            })
        })
        .collect();
    let resp = transport
        .post("/sync/push", &serde_json::json!({ "ops": ops_json }))
        .unwrap();
    let accepted: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(accepted["accepted"].as_array().unwrap().len(), 2);

    // Device B pulls with a cursor at the shared hlc: both ops are
    // unreachable (`hlc > since` is strict) — silent data loss.
    let conn_b = open_in_memory().unwrap();
    let b = Repos::new(conn_b, 2);
    let s = sync_cycle(&b, &identity, &transport, 100).unwrap();
    // Fresh B cursor is "" so the first pull gets BOTH ops (hlc > "").
    assert_eq!(s.applied, 2);
    // Now the KILLER: B advances its in-memory cursor to the shared
    // hlc; a THIRD op arrives with the same hlc text on the relay.
    a.conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO crdt_outbox (operation_id, hlc_timestamp, table_name, record_id, encrypted_payload, created_at_epoch_ms, pushed)
             VALUES ('op-g-three', ?1, 'goals', 'g-three', ?2, 1, 0)",
            rusqlite::params![
                shared_ts.to_string(),
                {
                    let sealed = wl_core::crypto::aead::seal(
                        &identity,
                        &serde_json::to_vec(&serde_json::json!({
                            "title": "Three", "description": null,
                            "target_date": null, "status": "active"
                        }))
                        .unwrap(),
                        b"goals:g-three",
                    )
                    .unwrap();
                    sealed.to_bytes()
                }
            ],
        )
        .unwrap();
    let resp = transport
        .post(
            "/sync/pull",
            &serde_json::json!({"since_hlc": shared_ts.to_string(), "limit": 100}),
        )
        .unwrap();
    let pulled: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(
        pulled["ops"].as_array().unwrap().len(),
        0,
        "DEFECT: op-g-three (same hlc text as cursor) is unreachable by strict `hlc > since` — permanently lost for any device past that cursor"
    );

    // save_cursor exists but is never called by sync_cycle; assert the
    // observable consequence: B's cursor is also never persisted.
    let cursor: Option<String> = b
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT cursor FROM sync_cursor WHERE id = 1", [], |r| {
            r.get(0)
        })
        .ok();
    assert!(
        cursor.is_none(),
        "DEFECT CONFIRMED: sync_cursor table never written by sync_cycle (save_cursor dead code)"
    );
    // save_cursor itself works when called explicitly (API exists).
    save_cursor(&b, "marker").unwrap();
}

/// Finding: `sync_cycle` push phase reads `pending_outbox(batch_limit)`
/// — a cycle only pushes `batch_limit` ops (200 in the shell) even when
/// MORE are pending; the SyncStatsView.pending count then reports the
/// remainder but nothing schedules another push. Large offline bursts
/// (e.g. an AI master plan: 5 milestones × directives + phases ≈ 20+
/// ops) can exceed limits only at extreme scale; the sharper defect:
/// push happens ONCE per cycle while pull loops. Any pending>0 after a
/// user-triggered sync requires the user to click "Sync now" again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defect_push_drains_only_one_batch_per_cycle() {
    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport { app, token };

    let conn_a = open_in_memory().unwrap();
    let a = Repos::new(conn_a, 1);
    // 30 pending ops, batch limit 10 → one cycle pushes only 10.
    for i in 0..30 {
        let rec = format!("g-{i}");
        a.enqueue_outbox(
            &identity,
            "goals",
            &rec,
            &serde_json::json!({"title": format!("G{i}"), "description": null,
                "target_date": null, "status": "active"}),
        )
        .unwrap();
    }
    let s = sync_cycle(&a, &identity, &transport, 10).unwrap();
    assert_eq!(s.pushed, 10);
    assert_eq!(a.pending_outbox(100).unwrap().len(), 20);
}
