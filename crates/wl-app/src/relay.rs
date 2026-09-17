//! Relay session handshake (PRD §3.2): challenge → Ed25519 sign →
//! verify. Produces the short-lived bearer token that push/pull calls
//! attach. Tokens expire server-side (1h TTL) and die with relay
//! restarts, so callers treat HTTP 401 as "re-handshake and retry once".

use wl_core::crypto::identity::Identity;

use crate::error::{ShellError, ShellResult};

/// Performs the full handshake synchronously. Internals drive a nested
/// current-thread runtime via block_on — therefore this MUST only run on
/// a thread with no Tokio runtime context (production callers wrap it in
/// `tokio::task::spawn_blocking`; see `relay_authenticate`/`sync_now`).
/// Calling it directly from async code panics by design (fail fast, not
/// silent deadlock).
pub fn handshake(base: &str, identity: &Identity) -> ShellResult<wl_protocol::SessionToken> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| ShellError::Relay(format!("runtime: {e}")))?;
    rt.block_on(handshake_async(base, identity))
}

async fn handshake_async(
    base: &str,
    identity: &Identity,
) -> ShellResult<wl_protocol::SessionToken> {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| ShellError::Relay(format!("HTTP client: {e}")))?;
    let public_key = identity.public_key_hex();

    // 1. Unguessable challenge (also registers the account server-side).
    let challenge: wl_protocol::Challenge = post_json(
        &client,
        format!("{base}/auth/challenge"),
        vec![],
        &wl_protocol::ChallengeRequest {
            public_key: public_key.clone(),
        },
    )
    .await?;

    // 2. Sign nonce ‖ expiry with the local Ed25519 key (never leaves).
    let payload = wl_protocol::challenge_signing_payload(&challenge.nonce, challenge.expires_at);
    let sig = identity.sign(&payload);

    // 3. Verify → session token. Challenge parts travel in headers
    // (relay contract), signature hex in the body.
    let session: wl_protocol::SessionToken = post_json(
        &client,
        format!("{base}/auth/verify"),
        vec![
            ("x-nonce", challenge.nonce.clone()),
            ("x-expires", challenge.expires_at.to_string()),
        ],
        &wl_protocol::VerifyRequest {
            public_key,
            signature: hex::encode(sig),
        },
    )
    .await?;
    Ok(session)
}

async fn post_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: String,
    headers: Vec<(&str, String)>,
    body: &impl serde::Serialize,
) -> ShellResult<T> {
    let mut req = client.post(url).json(body);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| ShellError::Relay(format!("request failed: {e}")))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| ShellError::Relay(format!("read body: {e}")))?;
    if !status.is_success() {
        // Same envelope as ReqwestTransport so 401 detection is uniform.
        return Err(ShellError::Relay(format!("HTTP {status}: {text}")));
    }
    serde_json::from_str(&text).map_err(|e| ShellError::Relay(format!("decode: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Spins up the REAL relay on an ephemeral port and runs the exact
    /// shell handshake against it over TCP (Item 1 regression test —
    /// proves wire compatibility, not just router logic). The handshake
    /// itself runs on a blocking thread, mirroring production's
    /// spawn_blocking discipline (nested block_on panics on async threads).
    #[tokio::test]
    async fn handshake_against_live_relay() {
        let state = Arc::new(wl_relay::AppStateForTest {
            auth: wl_relay::AuthForTest::new(),
            blobs: Box::new(wl_relay::SqliteForTest::open_in_memory().unwrap()),
        });
        let app = wl_relay::router_for_test(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let identity = Identity::generate().unwrap();
        let base = format!("http://{addr}");
        let base2 = base.clone();
        let session = tokio::task::spawn_blocking(move || handshake(&base2, &identity))
            .await
            .unwrap()
            .expect("handshake succeeds");
        assert!(!session.token.is_empty());
        assert!(session.expires_at > 0);

        // The minted token authenticates a real sync endpoint.
        let authed: serde_json::Value = {
            let client = reqwest::Client::new();
            let resp = client
                .post(format!("{base}/sync/pull"))
                .header("authorization", format!("Bearer {}", session.token))
                .json(&wl_protocol::PullRequest {
                    since_hlc: String::new(),
                    since_op_id: String::new(),
                    limit: 10,
                })
                .send()
                .await
                .unwrap();
            assert!(resp.status().is_success());
            resp.json().await.unwrap()
        };
        assert_eq!(authed["exhausted"], serde_json::Value::Bool(true));
    }

    #[test]
    fn handshake_fails_closed_on_unreachable_relay() {
        let identity = Identity::generate().unwrap();
        // Port 1 is unroutable — must error, never panic or hang long.
        let err = handshake("http://127.0.0.1:1", &identity).unwrap_err();
        assert!(matches!(err, ShellError::Relay(_)));
    }
}
