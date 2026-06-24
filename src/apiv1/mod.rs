use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::{Json, Router};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use crate::db::{AppState, device::Device, log::Log};
use uuid::Uuid;
use crate::db::device::{generate_auth_token, get_device_by_raw_token, insert_device, update_last_sync_id};
use crate::{error, info};
use anyhow::{anyhow, Result};
use crate::db::log::{delete_log, insert_log};

pub async fn check() -> &'static str {
    "Time Tracker Backend v1"
}
#[derive(Deserialize)]
 struct RegisterPayload {
    name: String,
}

#[derive(Serialize)]
struct RegisterReturn{
    uuid:Uuid,
    token:String,
}

async fn register(State(state):State<AppState>,Json(pay_load):Json<RegisterPayload>)->Result<(StatusCode,Json<RegisterReturn>), (StatusCode, String)>{
    let token = generate_auth_token().map_err(internal_error)?;
    let new_device = Device::new(pay_load.name, &token).map_err(internal_error)?;
    insert_device(&state.pool, new_device.clone()).await.map_err(internal_error)?;
    Ok((StatusCode::OK, Json(RegisterReturn{
        uuid: new_device.uuid,
        token,
        }))
    )

}

#[derive(Deserialize)]
 struct LogPayload {
     token: String,
     logs:Vec<Log>,
}

//
async fn upload_all_logs(State(state): State<AppState>, Json(payload): Json<LogPayload>) -> Result<StatusCode, (StatusCode, String)> {
    let pool = &state.pool;
    let  token= payload.token;

    let device = get_device_by_raw_token(pool, token).await.map_err(internal_error)?;
    let  logs= payload.logs;
    let mut tx = state.pool.begin().await.map_err(internal_error)?;
    let logs_len = logs.len().clone();
    let highest_log_id = logs
        .iter()
        .map(|log| log.id)
        .max()
        .ok_or_else(|| internal_error("no logs"))?;
    update_last_sync_id(pool, device.hash_token.clone(),highest_log_id).await.map_err(internal_error)?;

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


#[derive(Debug, Serialize, Deserialize)]
struct SyncPayload{
    token:String,
    logs:Vec<Log>,
    deleted_log_ids:Vec<i64>,
}
async fn sync(
    State(state): State<AppState>,
    Json(payload): Json<SyncPayload>,
) -> Result<StatusCode, (StatusCode, String)> {
    let pool = &state.pool;

    let device = get_device_by_raw_token(pool, payload.token)
        .await
        .map_err(internal_error)?;

    let device_uuid = device.uuid.to_string();

    let highest_log_id = payload.logs.iter().map(|log| log.id).max();

    let mut tx = state.pool.begin().await.map_err(internal_error)?;

    for log in payload.logs {
        sqlx::query(
            "INSERT OR IGNORE INTO logs
             (id, device_uuid, app, timestamp, duration)
             VALUES (?, ?, ?, ?, ?)",
        )
            .bind(log.id)
            .bind(&device_uuid)
            .bind(log.app)
            .bind(log.timestamp)
            .bind(log.duration)
            .execute(&mut *tx)
            .await
            .map_err(internal_error)?;
    }

    for id in payload.deleted_log_ids {
        sqlx::query(
            "DELETE FROM logs
             WHERE id = ? AND device_uuid = ?",
        )
            .bind(id)
            .bind(&device_uuid)
            .execute(&mut *tx)
            .await
            .map_err(internal_error)?;
    }

    if let Some(highest_log_id) = highest_log_id {
        sqlx::query(
            "UPDATE devices
             SET last_sync_id = ?
             WHERE uuid = ?",
        )
            .bind(highest_log_id)
            .bind(&device_uuid)
            .execute(&mut *tx)
            .await
            .map_err(internal_error)?;
    }

    tx.commit().await.map_err(internal_error)?;

    Ok(StatusCode::OK)
}

async fn get_devices(State(state): State<AppState>)-> Result<(StatusCode, Json<Vec<Device>>), (StatusCode, String)> {
    let db = &state.pool;
    let devices:Vec<Device> = sqlx::query_as("select name, uuid,last_sync_id from devices").fetch_all(db).await.map_err(internal_error)?;
    Ok((StatusCode::OK, Json(devices)))
}
async fn get_device_logs(
    State(state): State<AppState>,
    Path(device_uuid): Path<String>,
) -> Result<(StatusCode, Json<Vec<Log>>), (StatusCode, String)> {
    let exists: Option<String> = sqlx::query_scalar(
        "SELECT uuid FROM devices WHERE uuid = ?",
    )
        .bind(&device_uuid)
        .fetch_optional(&state.pool)
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
        .fetch_all(&state.pool)
        .await
        .map_err(internal_error)?;

    Ok((StatusCode::OK, Json(logs)))
}



//check  -. v1
// registier (name)=> token and uuid
// push_all_logs (deivce, logs)=> last sync log id
// sync (device, logs, delete logs id)-> last sync log id
// get devices () devices uuid and name
// get logs/deivce_uuid

pub fn v1_router(db:SqlitePool)->Router{
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);
    Router::new()
        .route("/check", get(check))
        .route("/register", post(register))
        .route("/upload_all_logs", post(upload_all_logs))
        .route("/sync", post(sync))
        .route("/devices", get(get_devices))
        .route("/devices/{device_uuid}", get(get_device_logs))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        //100 MB
        .layer(DefaultBodyLimit::max(100 * 1024 * 1024))
        .with_state(AppState { pool: db })

}

fn internal_error<E: std::fmt::Display>(err: E) -> (StatusCode, String) {
    error!("{}", err);
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}