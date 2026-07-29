mod db;
mod apiv1;
mod admin;
mod log;

use axum::{Router};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;
use anyhow::{Context, Result};

use crate::admin::router as admin_router;
use crate::apiv1::v1_router;
use crate::db::{check_state, AppState, DATABASE_URL};

#[tokio::main]
async fn main() -> Result<()> {
    log::init()?;
    info!("Logger initialized (logs/info.log, logs/error.log)");

    info!("Connecting to database at {}", DATABASE_URL);
    let db_options = SqliteConnectOptions::from_str(DATABASE_URL)?
        .create_if_missing(true)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let db = SqlitePoolOptions::new()
        .max_connections(10)
        .connect_with(db_options)
        .await?;
    info!("Database connection pool ready (max_connections=10)");
    check_state(&db).await?;
    info!("Database schema verified");

    let admin_password = std::env::var("TIME_TRACKER_ADMIN_PASSWORD")
        .context("TIME_TRACKER_ADMIN_PASSWORD must be set before starting the backend")?;
    if admin_password.trim().is_empty() {
        anyhow::bail!("TIME_TRACKER_ADMIN_PASSWORD cannot be empty");
    }

    let app_state = AppState { pool: db };
    let app = Router::new()
        .nest("/v1", v1_router(app_state.pool.clone()))
        .nest("/admin", admin_router(app_state, admin_password));

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("Listening on http://{}", addr);
    info!("Admin panel: http://localhost:3000/admin");

    axum::serve(listener, app).await?;
    Ok(())
}
