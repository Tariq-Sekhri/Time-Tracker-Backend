mod db;
mod apiv1;
mod log;

use axum::{Router};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::net::SocketAddr;
use std::str::FromStr;
use anyhow::Result;

use crate::apiv1::v1_router;
use crate::db::{check_state, DATABASE_URL};

#[tokio::main]
async fn main() -> Result<()> {

    tracing_subscriber::fmt::init();
    let db_options = SqliteConnectOptions::from_str(DATABASE_URL)?
        .create_if_missing(true);
    let db = SqlitePoolOptions::new()
        .max_connections(10)
        .connect_with(db_options)
        .await?; 
    check_state(&db).await?;

    let app_v1 = v1_router(db);
    let app = Router::new().nest("/v1", app_v1);


    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!("Starting Server on {}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}
