//! Worldline relay binary entry point (thin shell over the library).

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
    #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
    let blobs: Box<dyn BlobStore> = {
        let url = std::env::var("WL_RELAY_PG_URL")
            .expect("WL_RELAY_PG_URL required for the postgres backend");
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
