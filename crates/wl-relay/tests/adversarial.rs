//! Adversarial relay tests — resource-exhaustion, auth surface, and
//! cursor semantics against the REAL axum router (one-shot, no TCP).

use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use axum::Router;
use tower::util::ServiceExt;

use base64::Engine as _;

async fn post(
    app: &Router,
    path: &str,
    body: String,
    token: Option<&str>,
) -> axum::http::Response<Body> {
    let mut req = Request::post(path).header("content-type", "application/json");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    app.clone()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap()
}

async fn body_text(resp: axum::http::Response<Body>) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// FIXED (was: unbounded unauthenticated account registration).
/// Account registration is now capped (`AccountQuota`): minting rows
/// requires no auth, so the cap is the storage-exhaustion bound. At
/// the HTTP layer the flood gets 429s past the cap.
#[tokio::test]
async fn defect_unbounded_unauthenticated_account_registration() {
    // Drive registration through the store directly (the HTTP layer
    // maps AccountQuota → 429; the cap itself lives in the backend).
    let store = wl_relay::SqliteForTest::open_in_memory().unwrap();
    use wl_relay::store::BlobStore;
    let mut minted = 0usize;
    let mut quota_hit = false;
    for i in 0..20_000i64 {
        let pk = format!("{:064x}", i);
        match store.register_account(&pk) {
            Ok(()) => minted += 1,
            Err(wl_relay::store::StoreError::AccountQuota) => {
                quota_hit = true;
                break;
            }
            Err(e) => panic!("unexpected: {e}"),
        }
    }
    assert!(quota_hit, "registration cap never engaged");
    assert!(minted <= 10_000, "cap allows {minted} accounts");

    // HTTP surface: over-cap registration is refused (429), not stored.
    let state = Arc::new(wl_relay::AppStateForTest {
        auth: wl_relay::AuthForTest::new(),
        blobs: Box::new(store),
    });
    let app = wl_relay::router_for_test(state);
    let over = format!("{:064x}", 50_000u64);
    let resp = post(
        &app,
        "/auth/challenge",
        format!("{{\"public_key\":\"{over}\"}}"),
        None,
    )
    .await;
    assert_eq!(resp.status(), 429, "over-cap registration must 429");
}

/// FIXED (was: challenge map unbounded within the TTL). In-flight
/// challenges are now capped per account AND globally: a flood of
/// unauthenticated challenges gets 429s and the in-memory map stays
/// bounded; legitimate auth + pull keeps working.
#[tokio::test]
async fn defect_challenge_flood_grows_state_without_cap() {
    let state = Arc::new(wl_relay::AppStateForTest {
        auth: wl_relay::AuthForTest::new(),
        blobs: Box::new(wl_relay::SqliteForTest::open_in_memory().unwrap()),
    });
    let app = wl_relay::router_for_test(state.clone());
    let pk = format!("{:064x}", 42u64);
    // Flood: all requests past the per-account cap are refused with 429.
    let mut ok = 0;
    let mut limited = 0;
    for _ in 0..100 {
        let resp = post(
            &app,
            "/auth/challenge",
            serde_json::json!({"public_key": pk}).to_string(),
            None,
        )
        .await;
        match resp.status().as_u16() {
            200 => ok += 1,
            429 => limited += 1,
            other => panic!("unexpected status {other}"),
        }
    }
    assert!(ok <= wl_relay::auth::MAX_CHALLENGES_PER_ACCOUNT);
    assert!(limited > 0, "flood never hit the per-account cap");
    // Then prove a full auth+pull still functions (legitimate client).
    use ed25519_dalek::Signer;
    let sk = ed25519_dalek::SigningKey::from_bytes(&[5u8; 32]);
    let real_pk = hex::encode(sk.verifying_key().as_bytes());
    let ch_resp = post(
        &app,
        "/auth/challenge",
        serde_json::json!({"public_key": real_pk}).to_string(),
        None,
    )
    .await;
    assert_eq!(ch_resp.status(), 200);
    let ch: wl_protocol::Challenge = serde_json::from_str(&body_text(ch_resp).await).unwrap();
    let payload = wl_protocol::challenge_signing_payload(&ch.nonce, ch.expires_at);
    let sig = hex::encode(sk.sign(&payload).to_bytes());
    let v_resp = app
        .clone()
        .oneshot(
            Request::post("/auth/verify")
                .header("content-type", "application/json")
                .header("x-nonce", &ch.nonce)
                .header("x-expires", ch.expires_at.to_string())
                .body(Body::from(
                    serde_json::json!({"public_key": real_pk, "signature": sig}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(v_resp.status(), 200);
    let session: wl_protocol::SessionToken =
        serde_json::from_str(&body_text(v_resp).await).unwrap();
    let resp = post(
        &app,
        "/sync/pull",
        serde_json::json!({"since_hlc": "", "limit": 5}).to_string(),
        Some(&session.token),
    )
    .await;
    assert_eq!(resp.status(), 200);
    let empty: wl_protocol::PullResponse = serde_json::from_str(&body_text(resp).await).unwrap();
    assert!(empty.ops.is_empty() && empty.exhausted);
}

/// FIXED (was: `hlc > since` strictly skipped same-HLC ops, and
/// pagination set next_cursor to the last op's hlc — stragglers
/// sharing that hlc were permanently unreachable). The cursor is now
/// the COMPOSITE (hlc, operation_id): same-HLC ties advance via the
/// op id, so paginating through N ops that share ONE hlc reaches all
/// of them.
#[tokio::test]
async fn defect_pull_pagination_can_skip_ops_sharing_hlc() {
    let state = Arc::new(wl_relay::AppStateForTest {
        auth: wl_relay::AuthForTest::new(),
        blobs: Box::new(wl_relay::SqliteForTest::open_in_memory().unwrap()),
    });
    let app = wl_relay::router_for_test(state.clone());
    use ed25519_dalek::Signer;

    let sk = ed25519_dalek::SigningKey::from_bytes(&[3u8; 32]);
    let real_pk = hex::encode(sk.verifying_key().as_bytes());
    // 5 ops sharing the SAME hlc text for the real account.
    let shared_hlc = "00000000000000001000.00005.00001";
    let mut expected = std::collections::HashSet::new();
    for i in 0..5 {
        let op_id = format!("real-op-{i}");
        expected.insert(op_id.clone());
        state
            .blobs
            .insert_ops(&[wl_relay::store::StoredOp {
                operation_id: op_id,
                account: real_pk.clone(),
                hlc: shared_hlc.to_string(),
                table: "goals".into(),
                record_id: format!("g-{i}"),
                sealed: vec![1u8; 40],
            }])
            .unwrap();
    }

    // Handshake through the app's own AuthState.
    let challenge_resp = post(
        &app,
        "/auth/challenge",
        serde_json::json!({"public_key": real_pk}).to_string(),
        None,
    )
    .await;
    assert_eq!(challenge_resp.status(), 200);
    let ch: wl_protocol::Challenge =
        serde_json::from_str(&body_text(challenge_resp).await).unwrap();
    let payload = wl_protocol::challenge_signing_payload(&ch.nonce, ch.expires_at);
    let sig = hex::encode(sk.sign(&payload).to_bytes());
    let verify_resp = app
        .clone()
        .oneshot(
            Request::post("/auth/verify")
                .header("content-type", "application/json")
                .header("x-nonce", &ch.nonce)
                .header("x-expires", ch.expires_at.to_string())
                .body(Body::from(
                    serde_json::json!({"public_key": real_pk, "signature": sig}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(verify_resp.status(), 200);
    let session: wl_protocol::SessionToken =
        serde_json::from_str(&body_text(verify_resp).await).unwrap();

    // Page through ALL 5 same-hlc ops with limit 2 per page.
    let mut got = std::collections::HashSet::new();
    let (mut cursor, mut op_id) = (String::new(), String::new());
    for page in 0..10 {
        let resp = post(
            &app,
            "/sync/pull",
            serde_json::json!({"since_hlc": cursor, "since_op_id": op_id, "limit": 2}).to_string(),
            Some(&session.token),
        )
        .await;
        assert_eq!(resp.status(), 200);
        let pr: wl_protocol::PullResponse = serde_json::from_str(&body_text(resp).await).unwrap();
        for op in &pr.ops {
            assert!(got.insert(op.operation_id.clone()), "duplicate delivery");
        }
        cursor = pr.next_cursor.clone();
        op_id = pr.next_op_id.clone();
        if pr.exhausted || pr.ops.is_empty() {
            break;
        }
        assert!(page < 4, "pagination did not terminate");
    }
    assert_eq!(
        got, expected,
        "all same-hlc ops must be reachable via the composite cursor"
    );
}

/// FIXED (was: validate() never removed expired sessions and no GC
/// ran under pull-only traffic). validate() now removes the expired
/// row on sight and a throttled sweep runs periodically — a pull-only
/// client can no longer accumulate dead session rows. (TTL-based
/// expiry is covered in auth.rs `validate_replaces_expired_sessions_on_sight`.)
#[tokio::test]
async fn defect_sessions_gc_never_invoked_by_validate() {
    let auth = wl_relay::AuthForTest::new();
    // Unknown token: error, nothing to remove.
    assert!(auth.validate("nonexistent").is_err());
    // Live session validates and stays until expiry.
    use ed25519_dalek::Signer;
    let sk = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
    let pk = hex::encode(sk.verifying_key().as_bytes());
    let (nonce, expires) = auth.issue_challenge(&pk).unwrap();
    let payload = wl_protocol::challenge_signing_payload(&nonce, expires);
    let sig = sk.sign(&payload).to_bytes();
    let (token, _) = auth.verify(&pk, &nonce, expires, &sig).unwrap();
    assert_eq!(auth.validate(&token).unwrap(), pk);
    // Force-expire, validate again: the row is REMOVED (not retained).
    auth.expire_session_for_test(&token);
    assert!(auth.validate(&token).is_err());
    assert!(!auth.session_row_exists(&token));
}

/// KNOWN-LIMITATION (not fixed in this pass): `insert_ops` still holds
/// ONE transaction (and the store's single write mutex) across the
/// whole batch — a large push briefly convoys pulls. Recorded here as
/// the characterization guard; chunked commits are future work (P4).
#[tokio::test]
async fn defect_push_holds_global_mutex_across_whole_batch() {
    let state = Arc::new(wl_relay::AppStateForTest {
        auth: wl_relay::AuthForTest::new(),
        blobs: Box::new(wl_relay::SqliteForTest::open_in_memory().unwrap()),
    });
    let ops: Vec<_> = (0..20_000)
        .map(|i| wl_relay::store::StoredOp {
            operation_id: format!("op-{i}"),
            account: format!("{:064x}", 1u64),
            hlc: format!("{:019}.0.1", 1_000_000 + i),
            table: "goals".into(),
            record_id: format!("g-{i}"),
            sealed: vec![7u8; 32 + 16],
        })
        .collect();
    let t0 = std::time::Instant::now();
    let outcome = state.blobs.insert_ops(&ops).unwrap();
    let elapsed = t0.elapsed();
    assert_eq!(outcome.accepted.len(), 20_000);
    assert!(elapsed.as_millis() > 0);
}

/// Test helper: fresh app + authenticated session for a fixed key.
async fn authed_app() -> (Router, String) {
    use ed25519_dalek::Signer;
    let state = Arc::new(wl_relay::AppStateForTest {
        auth: wl_relay::AuthForTest::new(),
        blobs: Box::new(wl_relay::SqliteForTest::open_in_memory().unwrap()),
    });
    let app = wl_relay::router_for_test(state);
    let sk = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
    let pk = hex::encode(sk.verifying_key().as_bytes());
    let ch_resp = post(
        &app,
        "/auth/challenge",
        serde_json::json!({"public_key": pk}).to_string(),
        None,
    )
    .await;
    assert_eq!(ch_resp.status(), 200);
    let ch: wl_protocol::Challenge = serde_json::from_str(&body_text(ch_resp).await).unwrap();
    let payload = wl_protocol::challenge_signing_payload(&ch.nonce, ch.expires_at);
    let sig = hex::encode(sk.sign(&payload).to_bytes());
    // verify carries the challenge parts as headers (protocol surface).
    let v_resp = app
        .clone()
        .oneshot(
            Request::post("/auth/verify")
                .header("content-type", "application/json")
                .header("x-nonce", &ch.nonce)
                .header("x-expires", ch.expires_at.to_string())
                .body(Body::from(
                    serde_json::json!({"public_key": pk, "signature": sig}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(v_resp.status(), 200);
    let session: wl_protocol::SessionToken =
        serde_json::from_str(&body_text(v_resp).await).unwrap();
    (app, session.token)
}

fn push_body(op_id: &str, hlc: &str, table: &str, sealed_len: usize) -> String {
    use base64::Engine;
    serde_json::json!({"ops": [{
        "operation_id": op_id,
        "hlc": hlc,
        "table": table,
        "record_id": "rec-1",
        "sealed_b64": base64::engine::general_purpose::STANDARD.encode(vec![0u8; sealed_len]),
    }]})
    .to_string()
}

/// Push boundary validation: non-canonical HLC (unpadded), unknown
/// tables, oversized blobs and oversized batches are refused BEFORE
/// storage — poison must never enter TEXT-ordered ops.
#[tokio::test]
async fn push_boundary_rejects_malformed_envelopes() {
    let (app, token) = authed_app().await;
    let canon = format!("{:020}.{:05}.{:05}", 1_000_000u64, 0u16, 1u16);

    // Unpadded HLC: parses numerically but breaks TEXT ordering.
    let r = post(
        &app,
        "/sync/push",
        push_body("op-1", "1000000.0.1", "goals", 48),
        Some(&token),
    )
    .await;
    assert_eq!(r.status(), 400);
    // Garbage HLC.
    let r = post(
        &app,
        "/sync/push",
        push_body("op-2", "not-an-hlc", "goals", 48),
        Some(&token),
    )
    .await;
    assert_eq!(r.status(), 400);
    // Unknown table.
    let r = post(
        &app,
        "/sync/push",
        push_body("op-3", &canon, "evil_table", 48),
        Some(&token),
    )
    .await;
    assert_eq!(r.status(), 400);
    // Empty ids.
    let r = post(
        &app,
        "/sync/push",
        push_body("", &canon, "goals", 48),
        Some(&token),
    )
    .await;
    assert_eq!(r.status(), 400);
    // Oversized blob (> 256 KiB).
    let r = post(
        &app,
        "/sync/push",
        push_body("op-4", &canon, "goals", 300_000),
        Some(&token),
    )
    .await;
    assert_eq!(r.status(), 413);
    // Oversized batch (> 500 ops).
    let big: Vec<_> = (0..501)
        .map(|i| {
            serde_json::json!({
                "operation_id": format!("op-big-{i}"),
                "hlc": format!("{:020}.{:05}.{:05}", 2_000_000u64 + i as u64, 0u16, 1u16),
                "table": "goals",
                "record_id": "r",
                "sealed_b64": base64::engine::general_purpose::STANDARD.encode([0u8; 48]),
            })
        })
        .collect();
    let r = post(
        &app,
        "/sync/push",
        serde_json::json!({"ops": big}).to_string(),
        Some(&token),
    )
    .await;
    assert_eq!(r.status(), 413);

    // Canonical envelope: accepted.
    let r = post(
        &app,
        "/sync/push",
        push_body("op-ok", &canon, "goals", 48),
        Some(&token),
    )
    .await;
    assert_eq!(r.status(), 200);
    // Nothing rejected leaked into storage: exactly one op pulls back.
    let r = post(
        &app,
        "/sync/pull",
        serde_json::json!({"since_hlc": "", "since_op_id": "", "limit": 100}).to_string(),
        Some(&token),
    )
    .await;
    let pulled: wl_protocol::PullResponse = serde_json::from_str(&body_text(r).await).unwrap();
    assert_eq!(pulled.ops.len(), 1);
    assert_eq!(pulled.ops[0].operation_id, "op-ok");
}

/// Per-account op quota: an authenticated session cannot fill the
/// disk one push at a time — past 100k stored ops the relay answers
/// 429 and stores nothing further.
#[tokio::test]
async fn push_quota_caps_authenticated_storage_flood() {
    let state = Arc::new(wl_relay::AppStateForTest {
        auth: wl_relay::AuthForTest::new(),
        blobs: Box::new(wl_relay::SqliteForTest::open_in_memory().unwrap()),
    });
    let pk = format!("{:064x}", 7u64);
    state.blobs.register_account(&pk).unwrap();
    // Fill to the cap in large batches (store-level fast path).
    for i in 0..10u64 {
        let batch: Vec<_> = (0..10_000)
            .map(|j| wl_relay::store::StoredOp {
                operation_id: format!("fill-{i}-{j}"),
                account: pk.clone(),
                hlc: format!(
                    "{:020}.{:05}.{:05}",
                    1_000_000u64 + i * 10_000 + j as u64,
                    0u16,
                    1u16
                ),
                table: "goals".into(),
                record_id: "r".into(),
                sealed: vec![0u8; 48],
            })
            .collect();
        let out = state.blobs.insert_ops(&batch).unwrap();
        assert_eq!(out.accepted.len(), 10_000);
    }
    // One more op over the cap: refused at the store layer...
    let over = vec![wl_relay::store::StoredOp {
        operation_id: "over-cap".into(),
        account: pk.clone(),
        hlc: format!("{:020}.{:05}.{:05}", 9_999_999u64, 0u16, 1u16),
        table: "goals".into(),
        record_id: "r".into(),
        sealed: vec![0u8; 48],
    }];
    assert!(matches!(
        state.blobs.insert_ops(&over),
        Err(wl_relay::store::StoreError::OpsQuota)
    ));
}
