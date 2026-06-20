use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqlitePoolOptions;
use std::net::SocketAddr;
use sqlx::{FromRow, SqlitePool};
use tower_http::trace::TraceLayer;
use anyhow::Result;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};


static  DATABASE_URL:&str ="sqlite://app.db";
#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
}
#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct Device{
    pub id:i64,
    pub name: String,
    pub uuid:String,
}
#[derive(Debug, Serialize, Deserialize, FromRow)]
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

pub async fn get_devices(State(state): State<AppState>)->Result<(StatusCode, Json<Vec<Device>>), (StatusCode, String)>{
    let db = &state.db;
    let devices:Vec<Device> = sqlx::query_as("select * from devices").fetch_all(db).await.map_err(internal_error)?;
    Ok((StatusCode::FOUND, Json(devices)))
}
pub async fn get_device_logs(
    State(state): State<AppState>,
    Path(device_id): Path<i64>,
) -> Result<(StatusCode, Json<Vec<Log>>), (StatusCode, String)> {
    let exists: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM devices WHERE id = ?",
    )
        .bind(device_id)
        .fetch_optional(&state.db)
        .await
        .map_err(internal_error)?;

    if exists.is_none() {
        return Err((StatusCode::NOT_FOUND, "device not found".to_string()));
    }

    let logs: Vec<Log> = sqlx::query_as::<_, Log>(
        "SELECT id, device_id, app, timestamp, duration
         FROM logs
         WHERE device_id = ?",
    )
        .bind(device_id)
        .fetch_all(&state.db)
        .await
        .map_err(internal_error)?;

    Ok((StatusCode::OK, Json(logs)))
}

async fn check() -> &'static str {
    "Time Tracker Backend v1"
}



async fn delete_logs_by_ids(State(state):State<AppState>,device_id: Path<i64>, Json(ids):Json<Vec<i64>>)->Result<StatusCode, (StatusCode, String)>{
    let mut tx = state.db.begin().await.map_err(internal_error)?;
    for id in ids{
        sqlx::query("DELETE FROM logs WHERE id = ? and device_id = ?").bind(id).bind(*device_id).execute(&mut *tx).await.map_err(internal_error)?;
    }
    tx.commit().await.map_err(internal_error)?;
    Ok(StatusCode::OK)

}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let db = SqlitePoolOptions::new()
        .max_connections(10)
        .connect(DATABASE_URL)
        .await?;
    check_state(&db).await?;
    let app_v1= Router::new()
        .route("/check", get(check))
        .route("/upload_logs", post(upload_logs))
        .route("/devices", get(get_devices))
        .route("/devices/{device_id}", get(get_device_logs))
        .route("/devices/{device_id}/logs", delete(delete_logs_by_ids))
        .layer(TraceLayer::new_for_http())
        .with_state(AppState { db });
    let app = Router::new().nest("/v1", app_v1);
    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    println!("Starting Server on {}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}
