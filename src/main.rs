use axum::{routing::get, Json, Router};
use serde::Serialize;
use sqlx::sqlite::SqlitePoolOptions;
use std::net::SocketAddr;
use tower_http::trace::TraceLayer;

static  DATABASE_URL:&str ="sqlite://app.db";
#[derive(Clone)]
struct AppState {
    db: sqlx::SqlitePool,
}

#[derive(Serialize)]
struct HealthResponse {
    ok: bool,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { ok: true })
}

struct Log{
    
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let db = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(DATABASE_URL)
        .await?;

    let app = Router::new()
        .route("/health", get(health))
        .layer(TraceLayer::new_for_http())
        .with_state(AppState { db });

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    println!("Starting Server on {}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}