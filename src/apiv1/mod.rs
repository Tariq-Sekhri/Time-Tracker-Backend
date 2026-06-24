use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::{Json, Router};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tower_http::cors::{Any, CorsLayer};
use crate::db::{AppState, device::Device, log::Log};
use uuid::Uuid;
use crate::db::device::{generate_auth_token, get_device_by_raw_token, insert_device, update_last_sync_id, PubDevice};
use crate::{error, info};
use anyhow::Result;
use sqlx::Error as SqlxError;

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
    info!("Register request for device name={}", pay_load.name);
    let token = generate_auth_token().map_err(internal_error)?;
    let new_device = Device::new(pay_load.name, &token).map_err(internal_error)?;
    insert_device(&state.pool, new_device.clone()).await.map_err(internal_error)?;
    info!("Registered device uuid={} name={}", new_device.uuid, new_device.name);
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


async fn upload_all_logs(State(state): State<AppState>, Json(payload): Json<LogPayload>) -> Result<StatusCode, (StatusCode, String)> {
    let device = authenticate(&state.pool, payload.token).await?;
    let device_uuid = device.uuid.to_string();
    let logs = payload.logs;
    if logs.is_empty() {
        return Err(bad_request("no logs provided"));
    }

    let logs_len = logs.len();
    let highest_log_id = logs.iter().map(|log| log.id).max().unwrap();

    let mut tx = state.pool.begin().await.map_err(internal_error)?;

    for log in logs {
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

    update_last_sync_id(&mut *tx, &device_uuid, highest_log_id)
        .await
        .map_err(internal_error)?;

    tx.commit().await.map_err(internal_error)?;

    info!(
        "upload_all_logs device={} inserted={} highest_log_id={}",
        device.uuid,
        logs_len,
        highest_log_id
    );
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
    let device = authenticate(&state.pool, payload.token).await?;

    let device_uuid = device.uuid.to_string();
    info!(
        "sync device={} incoming_logs={} deleted_ids={}",
        device_uuid,
        payload.logs.len(),
        payload.deleted_log_ids.len()
    );

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
        update_last_sync_id(&mut *tx, &device_uuid, highest_log_id)
            .await
            .map_err(internal_error)?;
    }

    tx.commit().await.map_err(internal_error)?;

    if let Some(highest_log_id) = highest_log_id {
        info!("sync complete device={} last_sync_id={}", device_uuid, highest_log_id);
    } else {
        info!("sync complete device={} (no log updates)", device_uuid);
    }

    Ok(StatusCode::OK)
}



async fn get_devices(State(state): State<AppState>)-> Result<(StatusCode, Json<Vec<PubDevice>>), (StatusCode, String)> {
    let db = &state.pool;
    let devices:Vec<PubDevice> = sqlx::query_as("select name, uuid,last_sync_id from devices").fetch_all(db).await.map_err(internal_error)?;
    info!("get_devices returned {} device(s)", devices.len());
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
        error!("get_device_logs device={} not found", device_uuid);
        return Err((StatusCode::NOT_FOUND, "device not found".to_string()));
    }

    let logs: Vec<Log> = sqlx::query_as::<_, Log>(
        "SELECT id, device_uuid, app, timestamp, duration
         FROM logs
         WHERE device_uuid = ?",
    )
        .bind(&device_uuid)
        .fetch_all(&state.pool)
        .await
        .map_err(internal_error)?;

    info!("get_device_logs device={} returned {} log(s)", device_uuid, logs.len());
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
        .layer(DefaultBodyLimit::max(100 * 1024 * 1024))
        .with_state(AppState { pool: db })

}

fn internal_error<E: std::fmt::Display>(err: E) -> (StatusCode, String) {
    error!("internal server error: {}", err);
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}

fn bad_request(message: &str) -> (StatusCode, String) {
    error!("bad request: {}", message);
    (StatusCode::BAD_REQUEST, message.to_string())
}

async fn authenticate(pool: &SqlitePool, token: String) -> Result<Device, (StatusCode, String)> {
    get_device_by_raw_token(pool, token)
        .await
        .map_err(|err| {
            if matches!(err, SqlxError::RowNotFound) {
                error!("invalid auth token");
                (StatusCode::UNAUTHORIZED, "invalid token".to_string())
            } else {
                internal_error(err)
            }
        })
}