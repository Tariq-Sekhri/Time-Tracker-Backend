use axum::{Json, Router};
use serde::{Deserialize};
use sqlx::sqlite::SqlitePoolOptions;
use std::net::SocketAddr;
use sqlx::SqlitePool;
use tower_http::trace::TraceLayer;
use anyhow::Result;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;

static  DATABASE_URL:&str ="sqlite://app.db";
#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
}
#[derive(Deserialize)]
pub struct Device{
    pub id:i64,
    pub name: String,
    pub uuid:String,
}
#[derive(Deserialize)]
pub struct Log{
    pub id:i64,
    pub device_id:i64,
    pub app:String,
    pub timestamp:i64,
    pub duration:i64,
}

pub async fn create_devices_table(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS devices (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            uuid TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL
        )",
    )
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn create_log_table(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS logs (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        device_id INTEGER NOT NULL DEFAULT 0,
        app TEXT NOT NULL,
        timestamp INTEGER NOT NULL,
        duration INTEGER NOT NULL DEFAULT 0
    )",
    )
        .execute(pool)
        .await?;
    Ok(())
}

async fn check_state(pool: &SqlitePool)->Result<()>{
    // create tables
    create_log_table(pool).await?;
    create_devices_table(pool).await?;

    Ok(())
}

#[derive(Deserialize)]
pub struct LogPayload {
    pub device:Device,
    pub logs:Vec<Log>,
}



pub async fn upload_logs(State(state): State<AppState>, Json(payload): Json<LogPayload>)->Result<StatusCode, (StatusCode, String)>{
    let db = &state.db;
    let  device= payload.device;
    let  logs= payload.logs;

    sqlx::query("INSERT OR IGNORE INTO devices (id, name, uuid) VALUES (?, ?, ?)").bind(device.id).bind( device.name.clone()).bind(device.uuid.clone()).execute(db).await.map_err(internal_error)?;
    let mut tx = state.db.begin().await.map_err(internal_error)?;

    for log in logs {
        sqlx::query(
            "INSERT OR IGNORE INTO logs
         (id, device_id, app, timestamp, duration)
         VALUES (?, ?, ?, ?, ?)",
        )
            .bind(log.id)
            .bind(log.device_id)
            .bind(log.app)
            .bind(log.timestamp)
            .bind(log.duration)
            .execute(&mut *tx)
            .await.map_err(internal_error)?;
    }

    tx.commit().await.map_err(internal_error)?;


    Ok(StatusCode::CREATED)


}

pub fn internal_error<E: std::fmt::Display>(err: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let db = SqlitePoolOptions::new()
        .max_connections(10)
        .connect(DATABASE_URL)
        .await?;
    check_state(&db).await?;
    let app = Router::new()
        .route("/upload_logs", post(upload_logs))
        .layer(TraceLayer::new_for_http())
        .with_state(AppState { db });

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    println!("Starting Server on {}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}