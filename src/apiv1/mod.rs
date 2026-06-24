use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::{Json, Router};
use axum::routing::{delete, get, post};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use crate::db::{AppState, device::Device, log::Log};
use uuid::Uuid;
use crate::db::device::{generate_auth_token, insert_device};

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
    insert_device(&state.db, new_device.clone()).await.map_err(internal_error)?;
    Ok((StatusCode::OK, Json(RegisterReturn{
        uuid: new_device.uuid,
        token,
        }))
    )

}

#[derive(Deserialize)]
 struct LogPayload {
     device:Device,
     logs:Vec<Log>,
}


async fn upload_all_logs(State(state): State<AppState>, Json(payload): Json<LogPayload>) -> anyhow::Result<StatusCode, (StatusCode, String)> {
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

struct SyncPayload{
    device:Device,
    logs:Vec<Log>,
    deleted_log_ids:Vec<i64>,
}
async fn sync(State(state):State<AppState>, Json(payload):Json<SyncPayload>){

}

async fn get_devices(State(state): State<AppState>)-> anyhow::Result<(StatusCode, Json<Vec<Device>>), (StatusCode, String)> {
    let db = &state.db;
    let devices:Vec<Device> = sqlx::query_as("select * from devices").fetch_all(db).await.map_err(internal_error)?;
    Ok((StatusCode::OK, Json(devices)))
}
async fn get_device_logs(
    State(state): State<AppState>,
    Path(device_uuid): Path<String>,
) -> anyhow::Result<(StatusCode, Json<Vec<Log>>), (StatusCode, String)> {
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
        .route("/register", get(register))
        .route("/upload_logs", post(upload_all_logs))
        .route("/devices", get(get_devices))
        .route("/devices/{device_uuid}", get(get_device_logs))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        //100 MB
        .layer(DefaultBodyLimit::max(100 * 1024 * 1024))
        .with_state(AppState { db })

}

fn internal_error<E: std::fmt::Display>(err: E) -> (StatusCode, String) {
    eprintln!("{}", err);
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}