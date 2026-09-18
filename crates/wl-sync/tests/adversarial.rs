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
    // save_cursor itself works when called explicitly (API exists).
    save_cursor(&b, "marker", "").unwrap();
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
    let g = a.create_goal("Clock", None, None, None).unwrap();
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
    save_cursor(&r, "1000000.0.1", "").unwrap();
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
/// not drive unbounded blob decoding or enqueue unknown-table poison
/// past the client boundary.
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
    for (op, last_id) in [(oversized, "op-big"), (unknown, "op-unknown-table")] {
        let resp = wl_protocol::PullResponse {
            ops: vec![op],
            next_cursor: "00000000000000000001.00000.00001".into(),
            next_op_id: last_id.into(),
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
    }
}
