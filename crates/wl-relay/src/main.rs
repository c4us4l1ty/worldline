//! Worldline relay binary entry point (thin shell over the library).

// SYNC-3: exactly one storage backend must be selected. Before these
// guards, `--no-default-features` failed with a confusing
// `E0425: cannot find value 'blobs' in this scope` (nothing ever bound
// it), and `--features sqlite,postgres` silently picked SQLite while the
// operator believed they had deployed Postgres. Both are now build
// errors that say what to do.
#[cfg(not(any(feature = "sqlite", feature = "postgres")))]
compile_error!(
    "wl-relay requires a storage backend: build with the default `--features sqlite` \
     (zero-infrastructure dev) or `--no-default-features --features postgres` (production). \
     With neither, no backend is compiled and the relay cannot start."
);

#[cfg(all(feature = "sqlite", feature = "postgres"))]
compile_error!(
    "wl-relay: `sqlite` and `postgres` are mutually exclusive backends, but both features \
     are enabled — SQLite was being selected silently. For Postgres use \
     `--no-default-features --features postgres`."
);

use std::net::SocketAddr;
use std::sync::Arc;

use wl_relay::auth::AuthState;
use wl_relay::router;
#[cfg(feature = "sqlite")]
use wl_relay::store::sqlite_backend::SqliteStore;
use wl_relay::store::BlobStore;
use wl_relay::AppState;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let addr: SocketAddr = std::env::var("WL_RELAY_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8080".into())
        .parse()?;

    // The store is built BEFORE any runtime exists.
    //
    // `PostgresStore::open` is synchronous and drives sqlx through
    // `Runtime::block_on`, which tokio refuses to call from a thread
    // already inside a runtime ("Cannot start a runtime from within a
    // runtime"). Under `#[tokio::main]` this thread IS one, so the
    // production build — the one the `compile_error!` above points
    // operators at — panicked before it ever bound a socket. Constructing
    // the store first and only then entering a runtime for `serve` fixes
    // it without changing the store's sync API, which its per-request
    // `spawn_blocking` callers rely on.
    #[cfg(feature = "sqlite")]
    let db_path = std::env::var("WL_RELAY_DB").unwrap_or_else(|_| "wl-relay.sqlite".into());
    #[cfg(feature = "sqlite")]
    let blobs: Box<dyn BlobStore> = if db_path == ":memory:" {
        Box::new(SqliteStore::open_in_memory()?)
    } else {
        Box::new(SqliteStore::open(std::path::Path::new(&db_path))?)
    };
    // Exclusivity is guaranteed by the `compile_error!` above, so this
    // arm no longer needs `not(feature = "sqlite")`.
    #[cfg(feature = "postgres")]
    let blobs: Box<dyn BlobStore> = {
        let url = std::env::var("WL_RELAY_PG_URL").map_err(|_| {
            anyhow::anyhow!(
                "WL_RELAY_PG_URL is required for the postgres backend \
                 (e.g. postgres://worldline:worldline@localhost:5433/worldline_relay)"
            )
        })?;
        Box::new(wl_relay::store::postgres_backend::PostgresStore::open(
            &url,
        )?)
    };

    let state = Arc::new(AppState {
        auth: AuthState::new(),
        blobs,
    });
    let app = router(state);

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        tracing::info!("Worldline relay listening on {addr} (blind blob drop box)");
        // The accept loop, not `axum::serve`, because the two ceilings
        // this relay needs both sit BELOW the service layer.
        //
        // `TimeoutLayer` and `ConcurrencyLimitLayer` are tower layers:
        // they run after hyper has accepted the socket, spawned a task
        // and parsed a complete request head. A client that opens a
        // connection and dribbles one byte of header every 29 s never
        // completes a head, so it never reaches the service, never
        // acquires a permit and is never subject to the 30 s deadline —
        // it just parks a task and hyper's read buffer for as long as it
        // likes. Memory therefore grew with the number of open sockets,
        // and nothing in the process could see it. The per-request
        // limits stay exactly where they were; these two bound what
        // they cannot reach.
        let open = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        loop {
            let (stream, _peer) = match listener.accept().await {
                Ok(pair) => pair,
                // A failed accept (fd exhaustion, a peer that vanished
                // between SYN and accept) must not take the listener
                // down with it.
                Err(e) => {
                    tracing::warn!("relay accept failed: {e}");
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    continue;
                }
            };
            if open.load(std::sync::atomic::Ordering::Relaxed) >= wl_relay::MAX_CONNECTIONS {
                // At the ceiling: refuse at the socket. The client sees
                // a closed connection and retries, which is honest
                // backpressure — the alternative is accepting and then
                // holding, which is the memory growth this exists to
                // stop.
                drop(stream);
                continue;
            }
            open.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let app = app.clone();
            let open = Arc::clone(&open);
            tokio::spawn(async move {
                let service = hyper_util::service::TowerToHyperService::new(app);
                let mut builder = hyper_util::server::conn::auto::Builder::new(
                    hyper_util::rt::TokioExecutor::new(),
                );
                // Only the request HEAD is bounded. The whole connection
                // is not, so a long-lived keep-alive connection and a
                // 30 s upload both still work; a partial head is simply
                // dropped instead of parked.
                builder
                    .http1()
                    .header_read_timeout(wl_relay::HEADER_READ_DEADLINE);
                let _ = builder
                    .serve_connection_with_upgrades(hyper_util::rt::TokioIo::new(stream), service)
                    .await;
                open.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            });
        }
    })
}
