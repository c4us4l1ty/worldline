//! Adversarial sync-layer characterization tests — each `defect_*` test
//! locks in a verified bug from the battle-test report.

use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use axum::Router;
use rusqlite::OptionalExtension;
use tower::util::ServiceExt;

use wl_core::crypto::identity::Identity;
use wl_core::store::open_in_memory;
use wl_core::store::repo::Repos;
use wl_sync::sync::{sync_cycle, Transport};

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
            String::from_utf8(bytes.to_vec()).map_err(|e| e.to_string())
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
                    String::from_utf8(bytes.to_vec()).map_err(|e| e.to_string())
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

/// FIXED (was: cursor never persisted, every cycle re-pulled full history).
/// `sync_cycle` persists the composite `(hlc, op_id)` cursor after every
/// batch, so repeat cycles pull nothing new.
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
    let g = a
        .create_goal(
            "Solo",
            None,
            None,
            wl_core::domain::COMPLEXITY_DEFAULT,
            None,
        )
        .unwrap();
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

    // A second, third, and Nth cycle: cursor persisted, nothing re-pulled.
    let s2 = sync_cycle(&b, &identity, &transport, 100).unwrap();
    assert_eq!(
        s2.pulled, 0,
        "cursor must be persisted; repeat cycles pull nothing new"
    );
    assert_eq!(s2.applied, 0);
    let s3 = sync_cycle(&b, &identity, &transport, 100).unwrap();
    assert_eq!(s3.pulled, 0);
}

/// FIXED (was: blind upsert with no LWW check). `apply_op_to_db` guards
/// every upsert with `hlc_timestamp < op.hlc`, so a stale remote op never
/// overwrites newer local state — convergence matches the in-memory
/// `TableState` contract on every delivery order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defect_pull_overwrites_newer_local_state_no_lww_check() {
    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport { app, token };

    // Device A pushes a STALE goal status (older HLC).
    let conn_a = open_in_memory().unwrap();
    let a = Repos::new(conn_a, 1);
    let g = a
        .create_goal(
            "Race",
            None,
            None,
            wl_core::domain::COMPLEXITY_DEFAULT,
            None,
        )
        .unwrap();
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
    let g_local = b
        .create_goal(
            "Race",
            None,
            None,
            wl_core::domain::COMPLEXITY_DEFAULT,
            None,
        )
        .unwrap();
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
        "archived",
        "LWW must keep the newer local 'archived' state against the stale remote op"
    );
    // And the row's hlc_timestamp is still the newer local one.
    assert!(after.hlc_timestamp >= newer_ts);
}

/// FIXED (was: TEXT lexicographic order inverted at digit boundaries).
/// HLC text is fixed-width (`pt(20).ctr(5).dev(5)`), so relay TEXT
/// comparison/sort matches numeric HLC order at every boundary.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defect_counter_width_text_order_diverges_from_numeric() {
    use wl_core::hlc::HlcTimestamp;
    // Fixed-width encodings of a digit-boundary pair.
    let a = HlcTimestamp::parse("900.10.1").unwrap();
    let b_ = HlcTimestamp::parse("900.9.1").unwrap();
    assert!(a > b_);
    let (sa, sb) = (a.to_string(), b_.to_string());
    // Relay-side comparison (SQLite TEXT) now agrees with numeric order.
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let ord: i64 = conn
        .query_row("SELECT ?1 > ?2", rusqlite::params![sa, sb], |r| r.get(0))
        .unwrap();
    assert_eq!(ord, 1, "fixed-width TEXT order must match numeric order");
}

/// FIXED (was: duplicates never drained the outbox). `sync_cycle` marks
/// both `accepted` AND `duplicates` pushed: a push that landed
/// server-side but whose response was lost drains on the retry instead
/// of re-pushing forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defected_outbox_never_drains_on_idempotent_repush() {
    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport { app, token };

    let conn_a = open_in_memory().unwrap();
    let a = Repos::new(conn_a, 1);
    let g = a
        .create_goal(
            "Drain",
            None,
            None,
            wl_core::domain::COMPLEXITY_DEFAULT,
            None,
        )
        .unwrap();
    a.enqueue_outbox(
        &identity,
        "goals",
        &g.id,
        &serde_json::json!({"title": "Drain", "description": null,
            "target_date": null, "status": "active"}),
    )
    .unwrap();

    // Simulate: push succeeded server-side but the response was lost
    // (op already in relay, client still shows it pending). Push the
    // pending op's bytes directly, bypassing the outbox state machine.
    {
        let pending = a.pending_outbox(100).unwrap();
        assert_eq!(pending.len(), 1);
        let o = &pending[0];
        let body = serde_json::to_value(wl_protocol::PushRequest {
            ops: vec![wl_protocol::PushOp {
                operation_id: o.operation_id.clone(),
                hlc: o.hlc_timestamp.to_string(),
                table: o.table_name.clone(),
                record_id: o.record_id.clone(),
                sealed_b64: base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    &o.encrypted_payload,
                ),
            }],
        })
        .unwrap();
        let resp = transport.post("/sync/push", &body).unwrap();
        let decoded: wl_protocol::PushResponse = serde_json::from_str(&resp).unwrap();
        assert_eq!(decoded.accepted.len(), 1);
        // Local outbox untouched: still pending.
        assert_eq!(a.pending_outbox(100).unwrap().len(), 1);
    }

    // Next cycle: relay reports the op as a duplicate, which drains it
    // (marked pushed, then deleted — relay-durable rows are not kept).
    let s = sync_cycle(&a, &identity, &transport, 100).unwrap();
    assert_eq!(
        s.pushed, 1,
        "duplicate op must drain via the relay's duplicates list"
    );
    let pending = a.pending_outbox(100).unwrap();
    assert!(
        pending.is_empty(),
        "outbox must drain on idempotent re-push, got {} pending",
        pending.len()
    );
    // Relay-durable rows are deleted, not accumulated as pushed=1.
    let total: i64 = a
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM crdt_outbox", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 0);
    // And it stays drained on the NEXT cycle too.
    let s2 = sync_cycle(&a, &identity, &transport, 100).unwrap();
    assert_eq!(s2.pushed, 0);
    assert!(a.pending_outbox(100).unwrap().is_empty());
}

/// FIXED (was: strict `hlc > since` skipped same-HLC ops forever, and
/// `save_cursor` was dead code). The pull cursor is the composite
/// `(hlc, operation_id)` and `sync_cycle` persists it after every batch,
/// so same-HLC ties advance via the op id and no op is unreachable.
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
                            wl_core::crdt::routing_aad(
                                "goals",
                                rec,
                                &format!("op-{rec}"),
                                &shared_ts.to_string(),
                            )
                            .as_bytes(),
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
                        wl_core::crdt::routing_aad(
                            "goals",
                            "g-three",
                            "op-g-three",
                            &shared_ts.to_string(),
                        )
                        .as_bytes(),
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
            &serde_json::json!({"since_hlc": shared_ts.to_string(), "since_op_id": "", "limit": 100}),
        )
        .unwrap();
    let pulled: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(
        pulled["ops"].as_array().unwrap().len(),
        2,
        "both same-hlc ops must stay reachable via the composite (hlc, op_id) cursor"
    );

    // sync_cycle persists its cursor after every batch.
    let cursor: Option<String> = b
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT cursor FROM sync_cursor WHERE id = 1", [], |r| {
            r.get(0)
        })
        .ok();
    assert!(
        cursor.is_some(),
        "sync_cursor table must be written by sync_cycle"
    );
    // The persisted cursor is a real, canonical HLC — `save_cursor`
    // refuses anything the relay would reject, so a cycle can never
    // wedge itself on a cursor it made up.
    let stored: String = b
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT cursor FROM sync_cursor WHERE id = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        wl_core::hlc::HlcTimestamp::parse(&stored)
            .unwrap()
            .to_string(),
        stored,
        "cursor must be canonical, got {stored:?}"
    );
}

/// FIXED (was: one push batch per cycle). `sync_cycle` drains the outbox
/// in a loop, so a 30-op offline burst with batch limit 10 pushes all 30
/// in one cycle instead of forcing repeated "Sync now" clicks.
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
    assert_eq!(s.pushed, 30);
    assert!(a.pending_outbox(100).unwrap().is_empty());
}

/// FIXED (was: any undecryptable/unparseable pulled op aborted the whole
/// cycle *before* the cursor advanced — one crafted row bricked every
/// future pull forever). Poison ops are now watermarked (quarantine)
/// and skipped; the cycle completes and the cursor advances past them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defect_poison_op_wedges_pull_cursor_forever() {
    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport {
        app: app.clone(),
        token,
    };

    // Device A: one good write, pushed.
    let conn_a = open_in_memory().unwrap();
    let a = Repos::new(conn_a, 1);
    let g = a
        .create_goal(
            "Solo",
            None,
            None,
            wl_core::domain::COMPLEXITY_DEFAULT,
            None,
        )
        .unwrap();
    a.enqueue_outbox(
        &identity,
        "goals",
        &g.id,
        &serde_json::json!({"id": g.id, "title": "Solo", "description": null,
            "target_date": null, "status": "active"}),
    )
    .unwrap();
    sync_cycle(&a, &identity, &transport, 100).unwrap();

    // Attacker (any authenticated client) stores a poison op: valid
    // base64 the relay accepts, but truncated below the 12+16 sealed
    // floor so no client can ever decrypt it.
    let poison_hlc = format!("{:020}.{:05}.{:05}", 1u64, 0u16, 9u16);
    let body = serde_json::json!({"ops": [{
        "operation_id": "op-poison-1",
        "hlc": poison_hlc,
        "table": "goals",
        "record_id": "g-poison",
        "sealed_b64": base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD, b"short"),
    }]});
    let resp = transport.post("/sync/push", &body).unwrap();
    let pushed: wl_protocol::PushResponse = serde_json::from_str(&resp).unwrap();
    assert_eq!(pushed.accepted, vec!["op-poison-1".to_string()]);

    // Device B pulls: good op applies, poison quarantines, cycle is Ok.
    let conn_b = open_in_memory().unwrap();
    let b = Repos::new(conn_b, 2);
    let s1 = sync_cycle(&b, &identity, &transport, 100).unwrap();
    assert_eq!(s1.pulled, 2);
    assert_eq!(s1.applied, 1);
    assert_eq!(s1.quarantined, 1);
    assert!(b.goal(&g.id).unwrap().is_some());

    // No wedge: the next cycle pulls nothing and also succeeds.
    let s2 = sync_cycle(&b, &identity, &transport, 100).unwrap();
    assert_eq!(s2.pulled, 0);
    assert_eq!(s2.quarantined, 0);
}

/// FIXED (was: pulled remote timestamps were never merged into the
/// local HLC — `observe_remote` built a throwaway clock). After a pull
/// from a fast peer, local ticks must exceed the remote timestamp or
/// LWW arbitration inverts on the next local write.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defect_pull_never_merges_remote_clock() {
    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport {
        app: app.clone(),
        token,
    };

    let conn_a = open_in_memory().unwrap();
    let a = Repos::new(conn_a, 1);
    let g = a
        .create_goal(
            "Clock",
            None,
            None,
            wl_core::domain::COMPLEXITY_DEFAULT,
            None,
        )
        .unwrap();
    a.enqueue_outbox(
        &identity,
        "goals",
        &g.id,
        &serde_json::json!({"id": g.id, "title": "Clock", "description": null,
            "target_date": null, "status": "active"}),
    )
    .unwrap();
    sync_cycle(&a, &identity, &transport, 100).unwrap();

    let conn_b = open_in_memory().unwrap();
    let b = Repos::new(conn_b, 2);
    let s = sync_cycle(&b, &identity, &transport, 100).unwrap();
    assert_eq!(s.applied, 1);
    let remote = b.goal(&g.id).unwrap().unwrap().hlc_timestamp;
    let local = b.hlc.now(b.device_id());
    assert!(
        local > remote,
        "local tick {local} must exceed pulled remote {remote}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tampered_pull_cursor_is_rejected_by_relay() {
    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport { app, token };

    let conn = open_in_memory().unwrap();
    let r = Repos::new(conn, 2);
    // Seed the bad cursor DIRECTLY: the client refuses to write one
    // (`save_cursor` validates), and the relay must still reject one
    // that arrives some other way — that is the property under test.
    r.conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO sync_cursor (id, cursor, op_id) VALUES (1, '1000000.0.1', '')
             ON CONFLICT(id) DO UPDATE SET cursor='1000000.0.1', op_id=''",
            [],
        )
        .unwrap();
    let err = sync_cycle(&r, &identity, &transport, 100).unwrap_err();
    let msg = match &err {
        wl_sync::sync::SyncError::Transport(m) => m.clone(),
        other => panic!("expected transport error, got {other:?}"),
    };
    assert!(msg.contains("400"), "relay must 400 the bad cursor: {msg}");
}
/// FIXED (was: an empty pull response merely had its next_cursor
/// syntax-checked — any value was accepted). A hostile relay answering
/// `ops: []` with an advanced cursor had it persisted by the cycle,
/// making every op between the old and forged position permanently
/// unreachable. The validator now requires an empty batch to restate
/// the cursor exactly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defect_empty_pull_jumps_cursor_permanently_skipping_ops() {
    struct JumpingTransport;
    impl Transport for JumpingTransport {
        fn post(&self, _path: &str, _body: &serde_json::Value) -> Result<String, String> {
            Ok(serde_json::json!({
                "ops": [],
                "next_cursor": format!("{:020}.00000.00001", 9_999_999u64),
                "next_op_id": "",
                "exhausted": true,
            })
            .to_string())
        }
    }
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let conn = open_in_memory().unwrap();
    let repos = Repos::new(conn, 1);
    let err = sync_cycle(&repos, &identity, &JumpingTransport, 100).unwrap_err();
    match &err {
        wl_sync::sync::SyncError::Protocol(m) => {
            assert!(m.contains("empty batch"), "got: {m}");
        }
        other => panic!("expected protocol error, got {other:?}"),
    }
    // The empty-batch error must abort BEFORE the cursor is persisted,
    // so the next real cycle re-pulls from the untouched position.
    let conn = repos.conn.lock().unwrap();
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT cursor, op_id FROM sync_cursor WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .unwrap();
    assert!(row.is_none(), "cursor must not be persisted: {row:?}");
}

/// FIXED (was: pulled ops were checked for header bounds only —
/// oversized sealed payloads and unknown tables passed the pull path
/// while the push path rejects them server-side). A hostile relay must
/// not drive unbounded blob decoding past the client boundary.
/// Envelope size bounds still abort; unknown TABLES no longer abort —
/// since B-008 they quarantine-skip (watermark + advance) so a
/// future-schema peer can never wedge sync (see
/// `unknown_table_quarantines_without_wedging_sync`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defect_pulled_ops_bypass_sealed_size_and_table_bounds() {
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let repos = Repos::new(open_in_memory().unwrap(), 1);
    let oversized = wl_protocol::PushOp {
        operation_id: "op-big".into(),
        hlc: "00000000000000000001.00000.00001".into(),
        table: "goals".into(),
        record_id: "r".into(),
        sealed_b64: "a".repeat(wl_protocol::MAX_SEALED_B64 + 1),
    };
    let unknown = wl_protocol::PushOp {
        operation_id: "op-unknown-table".into(),
        hlc: "00000000000000000001.00000.00001".into(),
        table: "not_a_table".into(),
        record_id: "r".into(),
        sealed_b64: String::new(),
    };
    // Oversized sealed payload: envelope bounds still abort the cycle.
    let resp = wl_protocol::PullResponse {
        ops: vec![oversized],
        next_cursor: "00000000000000000001.00000.00001".into(),
        next_op_id: "op-big".into(),
        exhausted: true,
    };
    let resp_json = serde_json::to_value(&resp).unwrap();
    struct HostileTransport {
        resp: serde_json::Value,
    }
    impl crate::Transport for HostileTransport {
        fn post(&self, path: &str, _body: &serde_json::Value) -> Result<String, String> {
            assert_eq!(path, "/sync/pull");
            Ok(self.resp.to_string())
        }
    }
    let tx = HostileTransport { resp: resp_json };
    let err = sync_cycle(&repos, &identity, &tx, 100).unwrap_err();
    match &err {
        wl_sync::sync::SyncError::Protocol(m) => {
            assert!(m.contains("bounds"), "got: {m}");
        }
        other => panic!("expected protocol error, got {other:?}"),
    }
    // Unknown table (valid envelope): B-008 quarantine-skip — the
    // cycle completes, the cursor advances past the op, nothing applies.
    let resp = wl_protocol::PullResponse {
        ops: vec![unknown],
        next_cursor: "00000000000000000001.00000.00001".into(),
        next_op_id: "op-unknown-table".into(),
        exhausted: true,
    };
    let resp_json = serde_json::to_value(&resp).unwrap();
    struct FutureSchemaTransport {
        resp: serde_json::Value,
    }
    impl crate::Transport for FutureSchemaTransport {
        fn post(&self, path: &str, _body: &serde_json::Value) -> Result<String, String> {
            assert_eq!(path, "/sync/pull");
            Ok(self.resp.to_string())
        }
    }
    let tx = FutureSchemaTransport { resp: resp_json };
    let stats = sync_cycle(&repos, &identity, &tx, 100).unwrap();
    assert_eq!((stats.pulled, stats.applied, stats.quarantined), (1, 0, 1));
    assert_eq!(stats.cursor, "00000000000000000001.00000.00001");
}

// ===========================================================================
// Battle-test campaign: one regression per fixed remote-exploit.
// ===========================================================================

/// A relay is an UNTRUSTED party. It sees `table`, `record_id`, `hlc` and
/// `operation_id` in cleartext — they are its routing key and its
/// pagination cursor — and it cannot read `sealed_b64`. All four are now
/// bound into the AEAD's associated data, so a rewritten header fails
/// Poly1305 instead of silently re-ordering the merge.
///
/// Before the fix the AAD was `table:record` alone, so a relay could
/// re-stamp any op to the top of the key space to make it win every
/// merge, relabel it to defeat the `(device, operation_id)` tie-break,
/// or rewind it so a real user's edit lost. None of that was detectable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defect_relay_rewriting_the_hlc_is_detected_and_quarantined() {
    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport { app, token };

    let repos = Repos::new(open_in_memory().unwrap(), 2);
    let g = repos
        .create_goal(
            "Mine",
            None,
            None,
            wl_core::domain::COMPLEXITY_DEFAULT,
            None,
        )
        .unwrap();

    // An honest op for `g`, sealed correctly.
    let honest_hlc = "00000000000000000009.00000.00001";
    let op_id = "op-honest";
    let sealed = wl_core::crypto::aead::seal(
        &identity,
        &serde_json::to_vec(&serde_json::json!({
            "title": "Original title", "description": null,
            "target_date": null, "status": "active"
        }))
        .unwrap(),
        wl_core::crdt::routing_aad("goals", &g.id, op_id, honest_hlc).as_bytes(),
    )
    .unwrap();
    // …then the relay rewrites ONLY the cleartext HLC, to the top of the
    // key space, so the op would beat every honest write forever.
    let forged_hlc = "00000000000000000099.00000.00001";
    let body = serde_json::json!({
        "ops": [{
            "operation_id": op_id,
            "hlc": forged_hlc,
            "table": "goals",
            "record_id": g.id,
            "sealed_b64": base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD, sealed.to_bytes()),
        }]
    });
    transport.post("/sync/push", &body).unwrap();

    let stats = sync_cycle(&repos, &identity, &transport, 100).unwrap();
    assert_eq!(
        (stats.pulled, stats.applied, stats.quarantined),
        (1, 0, 1),
        "a rewritten routing header must be quarantined, never merged"
    );
    let row = repos.goal(&g.id).unwrap().unwrap();
    assert_eq!(
        row.title, "Mine",
        "the local row must be untouched by an op whose header was forged"
    );
    // The cursor still advanced, so the poison cannot wedge sync.
    assert_eq!(stats.cursor, forged_hlc);
}

/// The same class, one field over: `operation_id` is the tie-break. A
/// relay that relabels it can decide which of two same-HLC ops wins.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defect_relay_relabelling_the_operation_id_is_detected() {
    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport { app, token };

    let repos = Repos::new(open_in_memory().unwrap(), 2);
    let g = repos
        .create_goal(
            "Mine",
            None,
            None,
            wl_core::domain::COMPLEXITY_DEFAULT,
            None,
        )
        .unwrap();
    let hlc = "00000000000000000009.00000.00001";
    let sealed = wl_core::crypto::aead::seal(
        &identity,
        &serde_json::to_vec(&serde_json::json!({
            "title": "Original", "description": null,
            "target_date": null, "status": "active"
        }))
        .unwrap(),
        wl_core::crdt::routing_aad("goals", &g.id, "op-real", hlc).as_bytes(),
    )
    .unwrap();
    transport
        .post(
            "/sync/push",
            &serde_json::json!({"ops": [{
                "operation_id": "op-zzz-relabelled", "hlc": hlc, "table": "goals",
                "record_id": g.id,
                "sealed_b64": base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD, sealed.to_bytes()),
            }]}),
        )
        .unwrap();
    let stats = sync_cycle(&repos, &identity, &transport, 100).unwrap();
    assert_eq!((stats.applied, stats.quarantined), (0, 1));
}

/// One pulled op whose HLC is `u64::MAX` must not be able to brick the
/// client.
///
/// The chain: `Hlc::observe` adopted the remote physical verbatim, the
/// head was persisted to `hlc_clock`, and the next local write carried
/// into an `.expect()` that PANICKED — in the write path, on every
/// device of the account, surviving restarts. One crafted op, no
/// authentication required, unrecoverable short of deleting the data dir.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defect_maxed_wire_hlc_cannot_brick_the_local_clock() {
    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport { app, token };

    let repos = Repos::new(open_in_memory().unwrap(), 2);
    let g = repos
        .create_goal(
            "Mine",
            None,
            None,
            wl_core::domain::COMPLEXITY_DEFAULT,
            None,
        )
        .unwrap();
    let maxed = "18446744073709551615.65535.00001";
    let fields = serde_json::json!({
        "title": "Poison", "description": null, "target_date": null, "status": "active"
    });
    // Sealed honestly under the maxed HLC: a hostile RELAY cannot forge
    // this (the AAD binds the HLC), but a hostile PEER sharing the
    // account key can, and the client must survive it either way.
    let sealed = wl_core::crypto::aead::seal(
        &identity,
        &serde_json::to_vec(&fields).unwrap(),
        wl_core::crdt::routing_aad("goals", &g.id, "op-maxed", maxed).as_bytes(),
    )
    .unwrap();
    transport
        .post(
            "/sync/push",
            &serde_json::json!({"ops": [{
                "operation_id": "op-maxed", "hlc": maxed, "table": "goals",
                "record_id": g.id,
                "sealed_b64": base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD, sealed.to_bytes()),
            }]}),
        )
        .unwrap();

    let stats = sync_cycle(&repos, &identity, &transport, 100).unwrap();
    assert_eq!((stats.pulled, stats.applied), (1, 1));

    // The data landed (fail-open on the payload) …
    assert_eq!(repos.goal(&g.id).unwrap().unwrap().title, "Poison");
    // … and the clock is still usable: more than a full counter's worth
    // of writes, strictly increasing, no panic.
    let mut last = repos.hlc.now(repos.device_id());
    for _ in 0..70_000 {
        let next = repos.hlc.now(repos.device_id());
        assert!(next >= last, "clock went backwards: {next:?} < {last:?}");
        last = next;
    }
    // A real write still lands.
    repos
        .create_goal(
            "After the storm",
            None,
            None,
            wl_core::domain::COMPLEXITY_DEFAULT,
            None,
        )
        .expect("local writes must still work");
    assert!(repos.goal(&g.id).unwrap().is_some());
}

/// A NOT NULL violation on any replicated table must quarantine, not
/// abort the cycle.
///
/// `is_fatal_store_error` allow-listed CHECK/FK/PK/UNIQUE but not NOT
/// NULL, and SQLite checks NOT NULL *before* CHECK — so a single sealed
/// `{}` was classified fatal, the cycle returned before `save_cursor`,
/// and the client re-pulled and re-failed on every future sync. A
/// permanent, unrecoverable wedge from one op.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defect_not_null_violation_quarantines_instead_of_wedging_sync() {
    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport { app, token };

    let repos = Repos::new(open_in_memory().unwrap(), 2);
    let op = |op_id: &str, hlc: &str, rec: &str, body: serde_json::Value| {
        let sealed = wl_core::crypto::aead::seal(
            &identity,
            &serde_json::to_vec(&body).unwrap(),
            wl_core::crdt::routing_aad("goals", rec, op_id, hlc).as_bytes(),
        )
        .unwrap();
        serde_json::json!({
            "operation_id": op_id, "hlc": hlc, "table": "goals", "record_id": rec,
            "sealed_b64": base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD, sealed.to_bytes()),
        })
    };
    // `goals.title` is NOT NULL; the payload omits it.
    let bad = op(
        "op-no-title",
        "00000000000000000001.00000.00001",
        "g-bad",
        serde_json::json!({"description": null, "status": "active"}),
    );
    // A good op AFTER it, so "the cursor advanced past the poison" is
    // observable rather than merely implied.
    let good = op(
        "op-good",
        "00000000000000000002.00000.00001",
        "g-good",
        serde_json::json!({
            "title": "Landed", "description": null,
            "target_date": null, "status": "active"
        }),
    );
    transport
        .post("/sync/push", &serde_json::json!({"ops": [bad, good]}))
        .unwrap();

    let stats = sync_cycle(&repos, &identity, &transport, 100).unwrap();
    assert_eq!(
        (stats.pulled, stats.applied, stats.quarantined),
        (2, 1, 1),
        "the poison must quarantine and the good op behind it must still land"
    );
    assert!(repos.goal("g-good").unwrap().is_some());
    assert!(repos.goal("g-bad").unwrap().is_none());
    assert!(!stats.cursor.is_empty(), "the cursor must have advanced");

    // A second cycle is a no-op, not a repeat failure.
    let again = sync_cycle(&repos, &identity, &transport, 100).unwrap();
    assert_eq!((again.pulled, again.quarantined), (0, 0));
}

/// A malformed check-in payload must NOT destroy the user's real
/// same-date check-in.
///
/// `apply_check_in_win` deleted (and tombstoned) the rows it was
/// displacing BEFORE inserting the winner, with every statement
/// autocommitting. A payload whose `outcome` was missing or out of the
/// CHECK list failed the INSERT — after the DELETE had already landed —
/// and the op was then quarantined, so the row could never come back on
/// that device while its peer still had it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defect_poison_check_in_cannot_destroy_a_real_one() {
    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport { app, token };

    let repos = Repos::new(open_in_memory().unwrap(), 2);
    // The user's genuine check-in for 2026-09-18.
    let mine = repos
        .upsert_check_in(
            "2026-09-18",
            wl_core::domain::CheckInOutcome::Done,
            Some("real"),
            None,
        )
        .unwrap();
    let before: i64 = repos
        .lock_conn()
        .query_row("SELECT COUNT(*) FROM check_ins", [], |r| r.get(0))
        .unwrap();
    assert_eq!(before, 1);

    // A peer op for the SAME date with an outcome outside the CHECK
    // list. Its HLC must BEAT the local check-in's, or arbitration
    // rejects it as stale and the delete path is never reached — the
    // property under test is the one that only fires on a WINNING op.
    let (head, _) = repos.hlc.head();
    let hlc = wl_core::hlc::HlcTimestamp {
        physical: head + 60_000_000_000,
        counter: 0,
        device: 9,
    }
    .to_string();
    let sealed = wl_core::crypto::aead::seal(
        &identity,
        &serde_json::to_vec(&serde_json::json!({
            "date": "2026-09-18", "outcome": "not-a-real-outcome", "note": null
        }))
        .unwrap(),
        wl_core::crdt::routing_aad("check_ins", &mine.id, "op-bad-checkin", &hlc).as_bytes(),
    )
    .unwrap();
    transport
        .post(
            "/sync/push",
            &serde_json::json!({"ops": [{
                "operation_id": "op-bad-checkin", "hlc": hlc, "table": "check_ins",
                "record_id": mine.id,
                "sealed_b64": base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD, sealed.to_bytes()),
            }]}),
        )
        .unwrap();

    let stats = sync_cycle(&repos, &identity, &transport, 100).unwrap();
    assert_eq!(stats.quarantined, 1, "the bad op must quarantine");
    let after: i64 = repos
        .lock_conn()
        .query_row("SELECT COUNT(*) FROM check_ins", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after, 1, "the real check-in must survive the poison op");
    let kept = repos
        .check_in_for_date("2026-09-18")
        .unwrap()
        .expect("the user's check-in is still there");
    assert_eq!(kept.outcome, wl_core::domain::CheckInOutcome::Done);
    assert_eq!(kept.note.as_deref(), Some("real"));
}

/// Deleting ONE phase on device A must not delete the others on device B.
///
/// Every step of a progressive directive shared the directive's record
/// id, so all of them were one merge record: the peer's apply ran
/// `DELETE FROM directive_phases WHERE directive_id = ?`, and removing
/// step 2 erased step 1 on every other device too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defect_deleting_one_phase_keeps_the_others_on_peers() {
    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport { app, token };

    let a = Repos::new(open_in_memory().unwrap(), 1);
    // Every writer needs the identity: without it nothing is enqueued
    // for the outbox and the peer's FK chain has no root.
    let g = a
        .create_goal(
            "Ship it",
            None,
            None,
            wl_core::domain::COMPLEXITY_DEFAULT,
            Some(&identity),
        )
        .unwrap();
    let m = a
        .create_milestone(&g.id, "M1", None, 0, Some(&identity))
        .unwrap();
    let d = a
        .create_directive(
            &m.id,
            "Progressive",
            None,
            30,
            2,
            "2026-09-13",
            None,
            &[("A".into(), None, 5), ("B".into(), None, 25)],
            Some(&identity),
        )
        .unwrap();
    assert_eq!(a.phases_for_directive(&d.id).unwrap().len(), 2);

    // A publishes, B pulls.
    let pushed = sync_cycle(&a, &identity, &transport, 100).unwrap();
    assert!(pushed.pushed > 0, "A's ops must reach the relay first");
    let b = Repos::new(open_in_memory().unwrap(), 2);
    let got = sync_cycle(&b, &identity, &transport, 100).unwrap();
    assert_eq!(got.quarantined, 0, "every phase op must apply cleanly");
    assert_eq!(
        b.phases_for_directive(&d.id).unwrap().len(),
        2,
        "both phases must replicate"
    );

    // A deletes ONLY step 2.
    a.delete_directive_phase(&d.id, 2, Some(&identity)).unwrap();
    assert_eq!(a.phases_for_directive(&d.id).unwrap().len(), 1);
    sync_cycle(&a, &identity, &transport, 100).unwrap();

    sync_cycle(&b, &identity, &transport, 100).unwrap();
    let on_b = b.phases_for_directive(&d.id).unwrap();
    assert_eq!(
        on_b.len(),
        1,
        "a one-step delete must not wipe the sibling phase on a peer: {on_b:?}"
    );
    assert_eq!(
        on_b[0].step, 1,
        "the surviving phase is the one not deleted"
    );
}

/// A local write must advance the record's merge head.
///
/// Only the delete writers did, so the head lagged behind the row on
/// every ordinary write. The pull side prefers the head over the row's
/// own HLC, so: peer op at T_r lands (head = T_r) → the user changes a
/// setting at T_l > T_r (row = T_l, head still T_r) → a third device's
/// op at T_m between them beats the head and blind-upserts the STALE
/// value over the user's newer edit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defect_stale_remote_op_cannot_clobber_a_newer_local_write() {
    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport { app, token };

    let repos = Repos::new(open_in_memory().unwrap(), 2);
    let mut settings = repos.settings().unwrap();
    settings.theme = "dark".into();
    repos.save_settings(&settings, None).unwrap();

    // Peer op lands at T_r.
    let push = |op_id: &str, hlc: &str, theme: &str| {
        let sealed = wl_core::crypto::aead::seal(
            &identity,
            &serde_json::to_vec(&serde_json::json!({
                "theme": theme, "ai_provider": null, "tier1_model": null,
                "tier2_model": null, "relay_url": null
            }))
            .unwrap(),
            wl_core::crdt::routing_aad("app_settings", "1", op_id, hlc).as_bytes(),
        )
        .unwrap();
        serde_json::json!({
            "operation_id": op_id, "hlc": hlc, "table": "app_settings",
            "record_id": "1",
            "sealed_b64": base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD, sealed.to_bytes()),
        })
    };
    transport
        .post(
            "/sync/push",
            &serde_json::json!({"ops": [
                push("op-peer-1", "00000000000000000001.00000.00001", "dark")
            ]}),
        )
        .unwrap();
    sync_cycle(&repos, &identity, &transport, 100).unwrap();
    assert_eq!(repos.settings().unwrap().theme, "dark");

    // The user switches to light locally — a strictly later write.
    let mut mine = repos.settings().unwrap();
    mine.theme = "light".into();
    repos.save_settings(&mine, None).unwrap();
    assert_eq!(repos.settings().unwrap().theme, "light");

    // A third device's op at an HLC BETWEEN the peer's and the user's.
    transport
        .post(
            "/sync/push",
            &serde_json::json!({"ops": [
                push("op-peer-2", "00000000000000000002.00000.00001", "dark")
            ]}),
        )
        .unwrap();
    sync_cycle(&repos, &identity, &transport, 100).unwrap();
    assert_eq!(
        repos.settings().unwrap().theme,
        "light",
        "an op older than the user's own edit must not win last-writer-wins"
    );
}
