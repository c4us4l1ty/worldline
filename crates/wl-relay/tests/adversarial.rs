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

/// SYNC-1: push now commits in chunks (`INSERT_CHUNK_SIZE`) instead of
/// one transaction for the whole batch, releasing the store's single
/// write mutex between chunks.
///
/// This test asserts the contracts chunking must *preserve*, all
/// deterministically. It deliberately does **not** assert on wall-clock
/// interleaving: proving the mutex is genuinely released mid-push is a
/// concurrency/latency property, and a timing assertion here would be
/// flaky on shared CI. That measurement belongs with the p95 latency
/// SLO in SHIP-3, not in a unit test.
#[tokio::test]
async fn defect_push_holds_global_mutex_across_whole_batch() {
    use wl_relay::store::BlobStore;
    let blobs = wl_relay::SqliteForTest::open_in_memory().unwrap();
    let account = format!("{:064x}", 1u64);
    blobs.register_account(&account).unwrap();

    // Comfortably more than one chunk, so the chunk loop actually runs.
    let n = 4_000usize;
    let ops: Vec<_> = (0..n)
        .map(|i| wl_relay::store::StoredOp {
            operation_id: format!("op-{i}"),
            account: account.clone(),
            hlc: format!("{:019}.0.1", 1_000_000 + i),
            table: "goals".into(),
            record_id: format!("g-{i}"),
            sealed: vec![7u8; 32 + 16],
        })
        .collect();

    // (1) A multi-chunk push still stores every op.
    let outcome = blobs.insert_ops(&ops).unwrap();
    assert_eq!(outcome.accepted.len(), n);
    assert!(outcome.duplicates.is_empty());

    // (2) Idempotence survives the chunk boundary: a re-push accepts
    // nothing and reports every op as a duplicate. This is the property
    // most at risk from splitting one transaction into many.
    let again = blobs.insert_ops(&ops).unwrap();
    assert_eq!(again.accepted.len(), 0, "re-push must accept nothing");
    assert_eq!(again.duplicates.len(), n, "all must be duplicates");

    // (3) A batch that is an exact multiple of the chunk size must not
    // drop or duplicate its final op — the classic off-by-one in a
    // `chunks()` loop.
    let exact = 256usize;
    let boundary: Vec<_> = (0..exact)
        .map(|i| wl_relay::store::StoredOp {
            operation_id: format!("b-{i}"),
            account: account.clone(),
            hlc: format!("{:019}.0.1", 3_000_000 + i),
            table: "goals".into(),
            record_id: format!("bg-{i}"),
            sealed: vec![7u8; 48],
        })
        .collect();
    let b = blobs.insert_ops(&boundary).unwrap();
    assert_eq!(b.accepted.len(), exact, "no op lost at a chunk boundary");

    // (4) Everything is actually durable and pullable.
    let pulled = blobs.pull_ops(&account, "", "", 10_000).unwrap();
    assert_eq!(pulled.len(), n + exact);
}

/// SYNC-1 companion: chunking must not weaken the per-account quota.
/// The cap is re-checked inside every chunk's transaction, so crossing
/// it partway through commits the prefix and then reports the cliff —
/// never silently accepting the whole batch.
#[tokio::test]
async fn chunked_push_still_enforces_the_per_account_cap() {
    use wl_relay::store::BlobStore;
    let blobs = wl_relay::SqliteForTest::open_in_memory().unwrap();
    let account = format!("{:064x}", 3u64);
    blobs.register_account(&account).unwrap();

    let chunk = 128usize;
    let mk = |from: i64, n: usize| -> Vec<wl_relay::store::StoredOp> {
        (0..n)
            .map(|i| wl_relay::store::StoredOp {
                operation_id: format!("op-{from}-{i}"),
                account: account.clone(),
                hlc: format!("{:019}.0.1", from + i as i64),
                table: "goals".into(),
                record_id: format!("g-{from}-{i}"),
                sealed: vec![7u8; 48],
            })
            .collect()
    };
    // Cap is 100_000. Fill to 99_968 (781 whole chunks), then push one
    // more chunk and expect the cliff.
    for b in 0..(99_968 / chunk) {
        blobs
            .insert_ops(&mk(b as i64 * chunk as i64, chunk))
            .unwrap();
    }
    let err = blobs.insert_ops(&mk(500_000, chunk)).unwrap_err();
    assert!(
        matches!(err, wl_relay::store::StoreError::OpsQuota),
        "expected the quota cliff, got {err:?}"
    );
    // The refused push must not have overshot the cap, and the account
    // must not be wedged: a push that still fits is accepted normally.
    // (The 128-op batch tripped on its *first* chunk — 99_968 + 128 >
    // 100_000 — so nothing was committed for it.)
    let fits = blobs.insert_ops(&mk(600_000, 1)).unwrap();
    assert_eq!(fits.accepted.len(), 1, "a fitting push still works");
    let one_too_many = blobs.insert_ops(&mk(700_000, 32));
    assert!(
        one_too_many.is_err(),
        "and the cap is still enforced immediately after"
    );
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

/// FIXED (was: ops keyed by operation_id alone — a GLOBAL constraint).
/// A client-minted operation id colliding across two accounts let the
/// first account's row swallow the second's and misreport it as a
/// duplicate, permanently losing that record's data. Deduplication is
/// now scoped per account: the same operation id stored by a second
/// account is an accepted, independent row.
#[tokio::test]
async fn operation_id_collisions_stay_scoped_per_account() {
    let state = Arc::new(wl_relay::AppStateForTest {
        auth: wl_relay::AuthForTest::new(),
        blobs: Box::new(wl_relay::SqliteForTest::open_in_memory().unwrap()),
    });
    let mk_op = |id: &str, account: u64, hlc: u64| wl_relay::store::StoredOp {
        operation_id: id.into(),
        account: format!("{account:064x}"),
        hlc: format!("{hlc:020}.00000.00001"),
        table: "goals".into(),
        record_id: "r".into(),
        sealed: vec![0u8; 48],
    };
    let a = mk_op("client-op-1", 1, 1_000_000);
    let b = mk_op("client-op-1", 2, 1_000_001);
    let out = state.blobs.insert_ops(&[a.clone(), b.clone()]).unwrap();
    assert_eq!(
        out.accepted,
        vec![a.operation_id.clone(), b.operation_id.clone()]
    );
    assert!(out.duplicates.is_empty());
    // Exact re-push stays idempotent within one account...
    let out = state.blobs.insert_ops(std::slice::from_ref(&a)).unwrap();
    assert_eq!(out.duplicates, vec![a.operation_id.clone()]);
    // ...and pulls return each account only its own row.
    let rows_a = state.blobs.pull_ops(&a.account, "", "", 100).unwrap();
    let rows_b = state.blobs.pull_ops(&b.account, "", "", 100).unwrap();
    assert_eq!(rows_a.len(), 1);
    assert_eq!(rows_b.len(), 1);
    assert_eq!(rows_a[0].hlc, a.hlc);
    assert_eq!(rows_b[0].hlc, b.hlc);
}

/// SYNC-4: the pull byte budget is the smallest binding limit, and it
/// is enforced by the relay.
///
/// Before this, the caps were incoherent as a product — 500 ops x 256 KiB
/// sealed is ~167 MiB of *legitimate* response — so no single ceiling
/// could be right, the 4 MiB `MAX_RESPONSE_BYTES` was referenced only by
/// a text validator, and `MAX_PULL_BYTES` was dead code whose definition
/// was its only occurrence. The relay now stops filling a batch at
/// `MAX_PULL_BYTES`, which makes the emitted response unconditionally
/// within budget.
#[tokio::test]
async fn pull_response_never_exceeds_the_byte_budget() {
    let (app, token) = authed_app().await;

    // Store enough max-sealed ops that a 500-op page cannot fit in the
    // budget. The push request cap (`MAX_REQUEST_BYTES`, 2 MiB) binds
    // first, so a max-sealed op (~341 KiB base64) means ~5 per push;
    // 42 ops ≈ 10 MiB stored, comfortably past the 8 MiB pull budget.
    let sealed_len = wl_protocol::MAX_SEALED_BYTES;
    let per_push = 5usize;
    for batch in 0..9i64 {
        let ops: Vec<_> = (0..per_push)
            .map(|i| {
                let n = batch * per_push as i64 + i as i64;
                serde_json::json!({
                    "operation_id": format!("op-{n}"),
                    "hlc": format!("{:020}.{:05}.{:05}", 1_000_000 + n, 0, 1),
                    "table": "goals",
                    "record_id": format!("rec-{n}"),
                    "sealed_b64": base64::engine::general_purpose::STANDARD
                        .encode(vec![0u8; sealed_len]),
                })
            })
            .collect();
        let resp = post(
            &app,
            "/sync/push",
            serde_json::json!({"ops": ops}).to_string(),
            Some(&token),
        )
        .await;
        assert_eq!(resp.status(), 200, "push batch {batch} must be accepted");
    }

    // Now pull with the maximum limit the protocol allows.
    let resp = post(
        &app,
        "/sync/pull",
        serde_json::json!({
            "since_hlc": "",
            "since_op_id": "",
            "limit": wl_protocol::MAX_BATCH_OPS,
        })
        .to_string(),
        Some(&token),
    )
    .await;
    assert_eq!(resp.status(), 200);

    let text = body_text(resp).await;
    // The serialized response must fit the budget the client will use to
    // read it. This is the end-to-end assertion the plan asked for: the
    // smallest binding limit, proven from the bytes on the wire.
    assert!(
        text.len() <= wl_protocol::MAX_PULL_BYTES,
        "pull response was {} bytes, over the {}-byte budget",
        text.len(),
        wl_protocol::MAX_PULL_BYTES
    );
    assert!(
        text.len() <= wl_protocol::MAX_RESPONSE_BYTES,
        "response must also fit MAX_RESPONSE_BYTES"
    );

    let pulled: wl_protocol::PullResponse = serde_json::from_str(&text).unwrap();
    // The budget truncated the page, so there IS more to pull and the
    // client is told so.
    assert!(
        !pulled.exhausted,
        "a truncated page must not claim exhausted"
    );
    // And pagination is lossless: the cursor is the last op included, so
    // resuming returns strictly later ops.
    let second = post(
        &app,
        "/sync/pull",
        serde_json::json!({
            "since_hlc": pulled.next_cursor,
            "since_op_id": pulled.next_op_id,
            "limit": wl_protocol::MAX_BATCH_OPS,
        })
        .to_string(),
        Some(&token),
    )
    .await;
    let page2: wl_protocol::PullResponse = serde_json::from_str(&body_text(second).await).unwrap();
    assert!(!page2.ops.is_empty(), "resuming must return more ops");
    let first_ids: std::collections::HashSet<_> =
        pulled.ops.iter().map(|o| &o.operation_id).collect();
    assert!(
        page2
            .ops
            .iter()
            .all(|o| !first_ids.contains(&o.operation_id)),
        "the second page must not repeat the first"
    );
}

/// SYNC-4: a page that fits the budget is returned whole and correctly
/// reports exhaustion — the budget must not silently truncate ordinary
/// traffic.
#[tokio::test]
async fn ordinary_pull_pages_are_not_truncated() {
    let (app, token) = authed_app().await;
    let ops: Vec<_> = (0..10i64)
        .map(|i| {
            serde_json::json!({
                "operation_id": format!("op-{i}"),
                "hlc": format!("{:020}.{:05}.{:05}", 1_000_000 + i, 0, 1),
                "table": "goals",
                "record_id": format!("rec-{i}"),
                "sealed_b64": base64::engine::general_purpose::STANDARD
                    .encode(vec![7u8; 256]),
            })
        })
        .collect();
    let resp = post(
        &app,
        "/sync/push",
        serde_json::json!({"ops": ops}).to_string(),
        Some(&token),
    )
    .await;
    assert_eq!(resp.status(), 200);

    let resp = post(
        &app,
        "/sync/pull",
        serde_json::json!({"since_hlc": "", "since_op_id": "", "limit": 100}).to_string(),
        Some(&token),
    )
    .await;
    let pulled: wl_protocol::PullResponse = serde_json::from_str(&body_text(resp).await).unwrap();
    assert_eq!(pulled.ops.len(), 10, "all 10 ops must come back");
    assert!(pulled.exhausted, "a short page means nothing remains");
}
