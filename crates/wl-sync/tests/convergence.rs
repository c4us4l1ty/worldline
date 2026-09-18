//! End-to-end sync tests: device A writes offline → syncs → device B
//! pulls → converges. Includes offline accumulation and convergence
//! latency budget (US-4: <300ms on high-speed connections).

use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use axum::Router;
use tower::util::ServiceExt;

use wl_core::crypto::identity::Identity;
use wl_core::store::open_in_memory;
use wl_core::store::repo::Repos;
use wl_sync::sync::{sync_cycle, Transport};

/// In-process transport: drives the relay router directly (no TCP —
/// measures the deterministic-merge path, not network RTT).
struct AxumTransport {
    app: Router,
    token: String,
}

impl Transport for AxumTransport {
    fn post(&self, path: &str, body: &serde_json::Value) -> Result<String, String> {
        // We may be called inside a tokio test worker: block_in_place
        // lets us drive the router on the current-thread runtime.
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
        // Try in-place blocking (multi-thread runtime); fall back to a
        // fresh current-thread runtime for single-threaded contexts.
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
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Boots an in-process relay with sqlite blobs.
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offline_same_date_check_ins_converge_on_device_tiebreak() {
    use wl_core::domain::CheckInOutcome;

    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport { app, token };
    let a = Repos::new(open_in_memory().unwrap(), 1);
    let b = Repos::new(open_in_memory().unwrap(), 2);
    for repos in [&a, &b] {
        repos.hlc.restore(8_000_000_000_000_000_000, 0);
    }
    let first = a
        .upsert_check_in("2026-09-18", CheckInOutcome::Partial, None, Some(&identity))
        .unwrap();
    let winner = b
        .upsert_check_in(
            "2026-09-18",
            CheckInOutcome::Done,
            Some("finished"),
            Some(&identity),
        )
        .unwrap();
    assert_eq!(first.hlc_timestamp.physical, winner.hlc_timestamp.physical);
    assert_eq!(first.hlc_timestamp.counter, winner.hlc_timestamp.counter);
    assert!(first.hlc_timestamp < winner.hlc_timestamp);
    assert_ne!(first.id, winner.id);

    sync_cycle(&a, &identity, &transport, 1).unwrap();
    sync_cycle(&b, &identity, &transport, 1).unwrap();
    sync_cycle(&a, &identity, &transport, 1).unwrap();
    for repos in [&a, &b] {
        assert_eq!(repos.recent_check_ins(10).unwrap(), vec![winner.clone()]);
    }
    let updated = a
        .upsert_check_in("2026-09-18", CheckInOutcome::Skipped, None, Some(&identity))
        .unwrap();
    sync_cycle(&a, &identity, &transport, 1).unwrap();
    sync_cycle(&b, &identity, &transport, 1).unwrap();
    assert_eq!(b.recent_check_ins(10).unwrap(), vec![updated]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offline_write_then_sync_converges_two_devices() {
    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport {
        app: app.clone(),
        token,
    };

    // ---- Device A: fully offline burst of writes ----
    let conn_a = open_in_memory().unwrap();
    let a = Repos::new(conn_a, 1);
    a.insert_identity(&identity, true, &[]).unwrap();
    let g = a
        .create_goal("Ship Worldline v0.1", None, Some("2026-10-01"), None)
        .unwrap();
    let ms = a
        .create_milestone(&g.id, "Crypto core", None, 0, None)
        .unwrap();
    let d = a
        .create_directive(
            &ms.id,
            "Write 300 words on Section 2.1",
            None,
            60,
            2,
            "2026-09-13",
            &[
                ("Signature first".into(), None, 5),
                ("Core loop".into(), None, 25),
            ],
            None,
        )
        .unwrap();
    // CRDT write-through: enqueue state of every mutated row.
    a.enqueue_outbox(
        &identity,
        "goals",
        &g.id,
        &serde_json::json!({"id": g.id, "title": "Ship Worldline v0.1", "description": null,
            "target_date": "2026-10-01", "status": "active"}),
    )
    .unwrap();
    a.enqueue_outbox(
        &identity,
        "milestones",
        &ms.id,
        &serde_json::json!({"id": ms.id, "goal_id": g.id, "title": "Crypto core",
            "description": null, "order_index": 0, "status": "pending"}),
    )
    .unwrap();
    a.enqueue_outbox(
        &identity,
        "directives",
        &d.id,
        &serde_json::json!({"id": d.id, "milestone_id": ms.id,
            "title": "Write 300 words on Section 2.1", "execution_context": null,
            "estimated_minutes": 60, "progressive_step": 1, "progressive_total": 2,
            "state": "queued", "scheduled_for_date": "2026-09-13"}),
    )
    .unwrap();
    assert_eq!(a.pending_outbox(100).unwrap().len(), 3);

    // ---- Reconnect: sync cycle pushes all 3 ----
    let stats = sync_cycle(&a, &identity, &transport, 100).unwrap();
    assert_eq!(stats.pushed, 3);
    assert_eq!(a.pending_outbox(100).unwrap().len(), 0);

    // ---- Device B (same mnemonic, fresh install) pulls everything ----
    let conn_b = open_in_memory().unwrap();
    let b = Repos::new(conn_b, 2);
    let start = std::time::Instant::now();
    let stats_b = sync_cycle(&b, &identity, &transport, 100).unwrap();
    let elapsed = start.elapsed();
    assert_eq!(stats_b.pulled, 3);
    assert_eq!(stats_b.applied, 3);
    // US-4 convergence budget (<300ms, excluding real network).
    assert!(elapsed.as_millis() < 300, "convergence took {elapsed:?}");

    // B now sees A's data verbatim.
    let goal_b = b.active_goal().unwrap().unwrap();
    assert_eq!(goal_b.title, "Ship Worldline v0.1");
    let ms_b = b.milestones_for_goal(&goal_b.id).unwrap();
    assert_eq!(ms_b.len(), 1);
    assert_eq!(ms_b[0].title, "Crypto core");
    let runnable = b.runnable_directives("2026-09-13").unwrap();
    assert_eq!(runnable.len(), 1);
    assert_eq!(runnable[0].title, "Write 300 words on Section 2.1");
    let phases = b.phases_for_directive(&runnable[0].id).unwrap();
    // Phases sync too? They weren't enqueued separately in this test —
    // only the directive row was. Phases are local to A for now.
    assert!(phases.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idempotent_repull_applies_nothing_twice() {
    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport { app, token };

    let conn_a = open_in_memory().unwrap();
    let a = Repos::new(conn_a, 1);
    let g = a.create_goal("Solo Goal", None, None, None).unwrap();
    a.enqueue_outbox(
        &identity,
        "goals",
        &g.id,
        &serde_json::json!({"id": g.id, "title": "Solo Goal", "description": null,
            "target_date": null, "status": "active"}),
    )
    .unwrap();
    sync_cycle(&a, &identity, &transport, 100).unwrap();

    let conn_b = open_in_memory().unwrap();
    let b = Repos::new(conn_b, 2);
    let s1 = sync_cycle(&b, &identity, &transport, 100).unwrap();
    assert_eq!(s1.applied, 1);
    // Second pull: op already applied → no-op.
    let s2 = sync_cycle(&b, &identity, &transport, 100).unwrap();
    assert_eq!(s2.applied, 0);
    let goals: i64 = b
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM goals", [], |r| r.get(0))
        .unwrap();
    assert_eq!(goals, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lww_conflict_resolves_identically_on_both_sides() {
    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport { app, token };

    let conn_a = open_in_memory().unwrap();
    let a = Repos::new(conn_a, 1);
    let conn_b = open_in_memory().unwrap();
    let b = Repos::new(conn_b, 2);

    let g = a.create_goal("Same Goal", None, None, None).unwrap();
    let goal_json = |status: &str, dev: u16| {
        serde_json::json!({"id": g.id, "title": "Same Goal", "description": null,
            "target_date": null, "status": status, "_dev": dev})
    };
    // A writes early; B writes later with a different status (higher HLC).
    a.enqueue_outbox(&identity, "goals", &g.id, &goal_json("active", 1))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    b.enqueue_outbox(&identity, "goals", &g.id, &goal_json("achieved", 2))
        .unwrap();

    // Both sync; pull each other's ops.
    sync_cycle(&a, &identity, &transport, 100).unwrap();
    sync_cycle(&b, &identity, &transport, 100).unwrap();
    sync_cycle(&a, &identity, &transport, 100).unwrap();
    sync_cycle(&b, &identity, &transport, 100).unwrap();

    let sa: String = a
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT status FROM goals", [], |r| r.get(0))
        .unwrap();
    let sb: String = b
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT status FROM goals", [], |r| r.get(0))
        .unwrap();
    // Later write wins on both replicas — deterministic convergence.
    assert_eq!(sa, sb, "replicas diverged: {sa} vs {sb}");
}

/// Item 5 contract: every repo mutation with an unlocked identity lands
/// in the outbox automatically (no manual `enqueue_outbox`), and a full
/// push→pull cycle converges ALL tables on a fresh device — including
/// phases, which the manual-enqueue era left local-only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_through_syncs_all_tables() {
    use wl_core::domain::{AppSettings, BailoutReason, CheckInOutcome, DirectiveState};

    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport { app, token };
    let id = Some(&identity);

    // ---- Device A: all writes go through mutators with identity ----
    let conn_a = open_in_memory().unwrap();
    let a = Repos::new(conn_a, 1);
    let g = a
        .create_goal("Write-through goal", Some("ctx"), Some("2026-10-01"), id)
        .unwrap();
    let ms = a
        .create_milestone(&g.id, "Milestone one", Some("desc"), 0, id)
        .unwrap();
    let d = a
        .create_directive(
            &ms.id,
            "Do the thing",
            Some("with care"),
            60,
            2,
            "2026-09-13",
            &[
                ("Phase one".into(), Some("first".into()), 5),
                ("Phase two".into(), None, 25),
            ],
            id,
        )
        .unwrap();
    a.set_directive_state(&d.id, DirectiveState::Active, id)
        .unwrap();
    assert!(a.advance_progressive_step(&d.id, id).unwrap());
    a.upsert_check_in("2026-09-13", CheckInOutcome::Partial, Some("half"), id)
        .unwrap();
    a.record_bailout(&d.id, BailoutReason::EnergyDepletion, Some("tired"), id)
        .unwrap();
    let settings = AppSettings {
        relay_url: Some("http://127.0.0.1:8080".into()),
        always_on_top: true,
        ..AppSettings::default()
    };
    a.save_settings(&settings, id).unwrap();

    // Every table above produced outbox ops with zero manual enqueues.
    let pending = a.pending_outbox(100).unwrap();
    let tables: std::collections::HashSet<&str> =
        pending.iter().map(|o| o.table_name.as_str()).collect();
    for t in [
        "goals",
        "milestones",
        "directives",
        "directive_phases",
        "check_ins",
        "bailouts",
        "app_settings",
    ] {
        assert!(tables.contains(t), "no write-through op for table {t}");
    }

    // ---- Push, then pull on a fresh device ----
    let pushed = sync_cycle(&a, &identity, &transport, 100).unwrap().pushed;
    assert!(
        pushed >= 7,
        "expected at least one op per table, got {pushed}"
    );

    let conn_b = open_in_memory().unwrap();
    let b = Repos::new(conn_b, 2);
    let stats = sync_cycle(&b, &identity, &transport, 100).unwrap();
    assert!(stats.applied >= 7);

    // ---- Device B converged on every table ----
    let goal_b = b.active_goal().unwrap().unwrap();
    assert_eq!(goal_b.title, "Write-through goal");
    assert_eq!(goal_b.target_date.as_deref(), Some("2026-10-01"));
    let ms_b = b.milestones_for_goal(&goal_b.id).unwrap();
    assert_eq!(ms_b.len(), 1);
    assert_eq!(ms_b[0].order_index, 0);

    let dir_b = b.directive(&d.id).unwrap().unwrap();
    assert_eq!(dir_b.state, DirectiveState::Active);
    assert_eq!(dir_b.progressive_step, 2);
    assert_eq!(dir_b.progressive_total, 2);
    assert_eq!(dir_b.estimated_minutes, 60);

    let phases_b = b.phases_for_directive(&d.id).unwrap();
    assert_eq!(phases_b.len(), 2);
    assert_eq!(phases_b[0].title, "Phase one");
    assert_eq!(phases_b[0].minutes, 5);

    let chk_b = b.check_in_for_date("2026-09-13").unwrap().unwrap();
    assert_eq!(chk_b.outcome, CheckInOutcome::Partial);

    let bail_count: i64 = b
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM bailouts WHERE reason = 'energy_depletion'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bail_count, 1);

    let settings_b = b.settings().unwrap();
    assert_eq!(
        settings_b.relay_url.as_deref(),
        Some("http://127.0.0.1:8080")
    );
    assert!(settings_b.always_on_top);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn poison_constraint_op_quarantines_without_wedging_sync() {
    use wl_core::domain::CheckInOutcome;

    let app = relay_app();
    let identity = Identity::from_phrase(PHRASE).unwrap();
    let token = authenticate(&app, &identity).await;
    let transport = AxumTransport { app, token };
    let a = Repos::new(open_in_memory().unwrap(), 1);
    let c1 = a
        .upsert_check_in("2026-09-17", CheckInOutcome::Done, None, Some(&identity))
        .unwrap();
    let c2 = a
        .upsert_check_in("2026-09-18", CheckInOutcome::Partial, None, Some(&identity))
        .unwrap();
    sync_cycle(&a, &identity, &transport, 100).unwrap();

    // Craft a same-id/different-date row move with a newer HLC: the LWW
    // id-guard engages, then the write hits UNIQUE(date). Deterministic
    // failure — retrying changes nothing — so it must quarantine.
    let ts = a.hlc.now(a.device_id());
    assert!(ts > c1.hlc_timestamp);
    let fields =
        serde_json::json!({"date": c2.date, "outcome": "done", "note": null});
    let aad = format!("check_ins:{}", c1.id);
    let sealed = wl_core::crypto::aead::seal(
        &identity,
        &serde_json::to_vec(&fields).unwrap(),
        aad.as_bytes(),
    )
    .unwrap();
    let body = serde_json::to_value(wl_protocol::PushRequest {
        ops: vec![wl_protocol::PushOp {
            operation_id: "op-poison-row-move".into(),
            hlc: ts.to_string(),
            table: "check_ins".into(),
            record_id: c1.id.clone(),
            sealed_b64: base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                sealed.to_bytes(),
            ),
        }],
    })
    .unwrap();
    let resp = transport.post("/sync/push", &body).unwrap();
    let decoded: wl_protocol::PushResponse = serde_json::from_str(&resp).unwrap();
    assert_eq!(decoded.accepted.len(), 1);

    let s = sync_cycle(&a, &identity, &transport, 100).unwrap();
    assert_eq!(s.quarantined, 1);
    // (The quarantine watermark itself is pruned with the advanced
    // cursor at cycle end — the relay paginates strictly after it, so
    // the op can never resurface.)
    // Loser rows untouched, winner rows intact.
    assert_eq!(a.check_in_for_date("2026-09-17").unwrap().unwrap().id, c1.id);
    assert_eq!(a.check_in_for_date("2026-09-18").unwrap().unwrap().id, c2.id);
    // The poison op never wedges later cycles.
    let s2 = sync_cycle(&a, &identity, &transport, 100).unwrap();
    assert_eq!((s2.pulled, s2.quarantined, s2.applied), (0, 0, 0));
}
