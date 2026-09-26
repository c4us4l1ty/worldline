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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    #[cfg(feature = "sqlite")]
    let db_path = std::env::var("WL_RELAY_DB").unwrap_or_else(|_| "wl-relay.sqlite".into());
    let addr: SocketAddr = std::env::var("WL_RELAY_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8080".into())
        .parse()?;

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
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Worldline relay listening on {addr} (blind blob drop box)");
    axum::serve(listener, app).await?;
    Ok(())
}
