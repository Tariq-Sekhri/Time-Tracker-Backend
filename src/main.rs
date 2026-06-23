use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteQueryResult};
use std::net::SocketAddr;
use std::str::FromStr;
use sqlx::{FromRow, SqlitePool};
use tower_http::trace::TraceLayer;
use anyhow::Result;
use axum::extract::{Path, State ,DefaultBodyLimit,};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use tower_http::cors::{Any, CorsLayer};

static  DATABASE_URL:&str ="sqlite://server.db";
#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
}
#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct Device {
    pub name: String,
    pub uuid: String,
}

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct Log{
    pub id:i64,
    pub device_uuid:String,
    pub app:String,
    pub timestamp:i64,
    pub duration:i64,
}


pub async fn create_devices_table(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS devices (
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
        device_uuid TEXT NOT NULL,
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

     let a = sqlx::query("INSERT OR IGNORE INTO devices (name, uuid) VALUES (?, ?)").bind( device.name.clone()).bind(device.uuid.clone()).execute(db).await.map_err(internal_error)?;

    let mut tx = state.db.begin().await.map_err(internal_error)?;
    let logs_len = logs.len().clone();
    for log in logs {
        sqlx::query(
            "INSERT OR IGNORE INTO logs
         (id, device_uuid, app, timestamp, duration)
         VALUES (?, ?, ?, ?, ?)",
        )
            .bind(log.id)
            .bind(&device.uuid)
            .bind(log.app)
            .bind(log.timestamp)
            .bind(log.duration)
            .execute(&mut *tx)
            .await.map_err(internal_error)?;
    }

    tx.commit().await.map_err(internal_error)?;

    println!("Device: {} Added {} logs", device.uuid , logs_len);
    Ok(StatusCode::OK)
}

pub fn internal_error<E: std::fmt::Display>(err: E) -> (StatusCode, String) {
    eprintln!("{}", err);
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}

pub async fn get_devices(State(state): State<AppState>)->Result<(StatusCode, Json<Vec<Device>>), (StatusCode, String)>{
    let db = &state.db;
    let devices:Vec<Device> = sqlx::query_as("select * from devices").fetch_all(db).await.map_err(internal_error)?;
    Ok((StatusCode::OK, Json(devices)))
}
pub async fn get_device_logs(
    State(state): State<AppState>,
    Path(device_uuid): Path<String>,
) -> Result<(StatusCode, Json<Vec<Log>>), (StatusCode, String)> {
    let exists: Option<String> = sqlx::query_scalar(
        "SELECT uuid FROM devices WHERE uuid = ?",
    )
        .bind(&device_uuid)
        .fetch_optional(&state.db)
        .await
        .map_err(internal_error)?;

    if exists.is_none() {
        return Err((StatusCode::NOT_FOUND, "device not found".to_string()));
    }

    let logs: Vec<Log> = sqlx::query_as::<_, Log>(
        "SELECT id, device_uuid, app, timestamp, duration
         FROM logs
         WHERE device_uuid = ?",
    )
        .bind(device_uuid)
        .fetch_all(&state.db)
        .await
        .map_err(internal_error)?;

    Ok((StatusCode::OK, Json(logs)))
}

async fn check() -> &'static str {
    "Time Tracker Backend v1"
}



async fn delete_logs_by_ids(State(state):State<AppState>, Path(device_uuid): Path<String>, Json(ids):Json<Vec<i64>>)->Result<StatusCode, (StatusCode, String)>{
    let exists: Option<String> = sqlx::query_scalar(
        "SELECT uuid FROM devices WHERE uuid = ?",
    )
        .bind(&device_uuid)
        .fetch_optional(&state.db)
        .await
        .map_err(internal_error)?;

    if exists.is_none() {
        return Err((StatusCode::NOT_FOUND, "device not found".to_string()));
    }

    let mut tx = state.db.begin().await.map_err(internal_error)?;
    for id in ids{
        sqlx::query("DELETE FROM logs WHERE id = ? AND device_uuid = ?").bind(id).bind(&device_uuid).execute(&mut *tx).await.map_err(internal_error)?;
    }
    tx.commit().await.map_err(internal_error)?;
    Ok(StatusCode::OK)

}

#[tokio::main]
async fn main() -> Result<()> {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    tracing_subscriber::fmt::init();
    let db_options = SqliteConnectOptions::from_str(DATABASE_URL)?
        .create_if_missing(true);
    let db = SqlitePoolOptions::new()
        .max_connections(10)
        .connect_with(db_options)
        .await?; 
    check_state(&db).await?;
    let app_v1= Router::new()
        .route("/check", get(check))
        .route("/upload_logs", post(upload_logs))
        .route("/devices", get(get_devices))
        .route("/devices/{device_uuid}", get(get_device_logs))
        .route("/devices/{device_uuid}/logs", delete(delete_logs_by_ids))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        //100 MB
        .layer(DefaultBodyLimit::max(100 * 1024 * 1024))
        .with_state(AppState { db });
    let app = Router::new().nest("/v1", app_v1);
    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    println!("Starting Server on {}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}
