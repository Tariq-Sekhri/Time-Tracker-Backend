use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::{Json, Router};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tower_http::cors::{Any, CorsLayer};
use crate::db::{AppState, device::Device, log::Log};
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
    uuid:String,
    token:String,
    is_active: bool,
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
        is_active: new_device.is_active,
        }))
    )

}

#[derive(Deserialize)]
 struct LogPayload {
     token: String,
     logs:Vec<Log>,
}


async fn upload_all_logs(State(state): State<AppState>, Json(payload): Json<LogPayload>) -> Result<StatusCode, (StatusCode, String)> {
    let device = authenticate_active(&state.pool, payload.token).await?;
    let device_uuid = device.uuid;
    let logs = payload.logs;
    if logs.is_empty() {
        return Err(bad_request("no logs provided"));
    }

    let logs_len = logs.len();
    let highest_log_id = logs.iter().map(|log| log.id).max().unwrap();

    let mut tx = state.pool.begin().await.map_err(internal_error)?;

    for log in logs {
        sqlx::query(
            "INSERT INTO logs
         (id, device_uuid, app, timestamp, duration)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(device_uuid, id) DO UPDATE SET
             app = excluded.app,
             timestamp = excluded.timestamp,
             duration = excluded.duration",
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
        device_uuid,
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
    let device = authenticate_active(&state.pool, payload.token).await?;

    let device_uuid = device.uuid;
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
            "INSERT INTO logs
             (id, device_uuid, app, timestamp, duration)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(device_uuid, id) DO UPDATE SET
                 app = excluded.app,
                 timestamp = excluded.timestamp,
                 duration = excluded.duration",
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



async fn get_devices(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<Vec<PubDevice>>), (StatusCode, String)> {
    authenticate_active_bearer(&state.pool, &headers).await?;
    let db = &state.pool;
    let devices:Vec<PubDevice> = sqlx::query_as(
        "select name, uuid, last_sync_id, is_active from devices where is_active = 1"
    ).fetch_all(db).await.map_err(internal_error)?;
    info!("get_devices returned {} device(s)", devices.len());
    Ok((StatusCode::OK, Json(devices)))
}
async fn get_device_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(device_uuid): Path<String>,
) -> Result<(StatusCode, Json<Vec<Log>>), (StatusCode, String)> {
    authenticate_active_bearer(&state.pool, &headers).await?;
    let exists: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM devices WHERE uuid = ? AND is_active = 1 LIMIT 1",
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
        "SELECT * FROM logs WHERE device_uuid = ?")
        .bind(&device_uuid)
        .fetch_all(&state.pool)
        .await
        .map_err(internal_error)?;

    info!("get_device_logs device={} returned {} log(s)", device_uuid, logs.len());
    Ok((StatusCode::OK, Json(logs)))
}


async fn get_devices_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(devices):Json<Vec<PubDevice>>
) -> Result<(StatusCode, Json<Vec<Log>>), (StatusCode, String)> {
    authenticate_active_bearer(&state.pool, &headers).await?;
    let mut logs:Vec<Log> = vec![];
    for device in &devices{


        logs.extend(sqlx::query_as::<_, Log>(
        "SELECT logs.* FROM logs
         JOIN devices ON devices.uuid = logs.device_uuid
         WHERE logs.device_uuid = ? AND logs.id > ? AND devices.is_active = 1")
        .bind(&device.uuid)
        .bind(&device.last_sync_id)
        .fetch_all(&state.pool)
        .await
        .map_err(internal_error)?);
    }

    info!("get_device_logs devices={:?} returned {} log(s)",devices.into_iter().map(|device| device.name), logs.len());
    Ok((StatusCode::OK, Json(logs)))
}




pub fn v1_router(db:SqlitePool)->Router{
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);
    Router::new()
        .route("/check", get(check))
        .route("/register", post(register))
        .route("/status", post(device_status))
        .route("/upload_all_logs", post(upload_all_logs))
        .route("/sync", post(sync))
        .route("/devices", get(get_devices))
        .route("/devices/", get(get_devices_logs))
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

async fn authenticate_active(pool: &SqlitePool, token: String) -> Result<Device, (StatusCode, String)> {
    let device = authenticate(pool, token).await?;
    if !device.is_active {
        error!("inactive device attempted a protected sync action uuid={}", device.uuid);
        return Err((
            StatusCode::FORBIDDEN,
            "device is waiting for admin approval".to_string(),
        ));
    }
    Ok(device)
}

async fn authenticate_active_bearer(
    pool: &SqlitePool,
    headers: &HeaderMap,
) -> Result<Device, (StatusCode, String)> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| !token.is_empty())
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "missing bearer token".to_string()))?;
    authenticate_active(pool, token.to_string()).await
}

#[derive(Deserialize)]
struct StatusPayload {
    token: String,
}

#[derive(Serialize)]
struct StatusReturn {
    uuid: String,
    name: String,
    is_active: bool,
}

async fn device_status(
    State(state): State<AppState>,
    Json(payload): Json<StatusPayload>,
) -> Result<Json<StatusReturn>, (StatusCode, String)> {
    let device = authenticate(&state.pool, payload.token).await?;
    Ok(Json(StatusReturn {
        uuid: device.uuid,
        name: device.name,
        is_active: device.is_active,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{check_state, device::Device};
    use sqlx::sqlite::SqlitePoolOptions;

    async fn test_state() -> (AppState, String, String) {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        check_state(&pool).await.unwrap();

        let token = "test-token".to_string();
        let device = Device::new("desktop".to_string(), &token).unwrap();
        let uuid = device.uuid.clone();
        insert_device(&pool, device).await.unwrap();
        sqlx::query("UPDATE devices SET is_active = 1 WHERE uuid = ?")
            .bind(&uuid)
            .execute(&pool)
            .await
            .unwrap();
        (AppState { pool }, token, uuid)
    }

    async fn insert_test_log(state: &AppState, uuid: &str, id: i64) {
        sqlx::query(
            "INSERT INTO logs (id, device_uuid, app, timestamp, duration)
             VALUES (?, ?, 'Editor', 100, 15)",
        )
        .bind(id)
        .bind(uuid)
        .execute(&state.pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn deletion_only_sync_is_accepted_and_idempotent() {
        let (state, token, uuid) = test_state().await;
        insert_test_log(&state, &uuid, 7).await;

        for _ in 0..2 {
            let result = sync(
                State(state.clone()),
                Json(SyncPayload {
                    token: token.clone(),
                    logs: vec![],
                    deleted_log_ids: vec![7],
                }),
            )
            .await;
            assert_eq!(result.unwrap(), StatusCode::OK);
        }

        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM logs WHERE device_uuid = ? AND id = 7")
                .bind(uuid)
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn sync_updates_an_existing_log_after_its_duration_changes() {
        let (state, token, uuid) = test_state().await;
        insert_test_log(&state, &uuid, 7).await;

        let result = sync(
            State(state.clone()),
            Json(SyncPayload {
                token,
                logs: vec![Log {
                    id: 7,
                    device_uuid: uuid.clone(),
                    app: "Editor".to_string(),
                    timestamp: 100,
                    duration: 45,
                }],
                deleted_log_ids: vec![],
            }),
        )
        .await;

        assert_eq!(result.unwrap(), StatusCode::OK);
        let duration: i64 = sqlx::query_scalar(
            "SELECT duration FROM logs WHERE device_uuid = ? AND id = 7",
        )
        .bind(uuid)
        .fetch_one(&state.pool)
        .await
        .unwrap();
        assert_eq!(duration, 45);
    }

    #[tokio::test]
    async fn invalid_token_cannot_insert_or_delete_logs() {
        let (state, _token, uuid) = test_state().await;
        insert_test_log(&state, &uuid, 7).await;

        let result = sync(
            State(state.clone()),
            Json(SyncPayload {
                token: "invalid".to_string(),
                logs: vec![Log {
                    id: 8,
                    device_uuid: uuid.clone(),
                    app: "Browser".to_string(),
                    timestamp: 200,
                    duration: 20,
                }],
                deleted_log_ids: vec![7],
            }),
        )
        .await;

        assert_eq!(result.unwrap_err().0, StatusCode::UNAUTHORIZED);
        let ids: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM logs WHERE device_uuid = ? ORDER BY id")
                .bind(uuid)
                .fetch_all(&state.pool)
                .await
                .unwrap();
        assert_eq!(ids, vec![7]);
    }

    #[tokio::test]
    async fn deleted_device_token_is_rejected() {
        let (state, token, uuid) = test_state().await;
        sqlx::query("DELETE FROM devices WHERE uuid = ?")
            .bind(uuid)
            .execute(&state.pool)
            .await
            .unwrap();

        let result = sync(
            State(state),
            Json(SyncPayload {
                token,
                logs: vec![],
                deleted_log_ids: vec![],
            }),
        )
        .await;
        assert_eq!(result.unwrap_err().0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn pending_device_can_check_status_but_cannot_sync() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        check_state(&pool).await.unwrap();
        let token = "pending-token".to_string();
        let device = Device::new("pending-desktop".to_string(), &token).unwrap();
        insert_device(&pool, device).await.unwrap();
        let state = AppState { pool };

        let Json(status) = device_status(
            State(state.clone()),
            Json(StatusPayload {
                token: token.clone(),
            }),
        )
        .await
        .unwrap();
        assert!(!status.is_active);

        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        let view_result = get_devices(State(state.clone()), headers).await;
        assert_eq!(view_result.unwrap_err().0, StatusCode::FORBIDDEN);

        let result = sync(
            State(state),
            Json(SyncPayload {
                token,
                logs: vec![],
                deleted_log_ids: vec![],
            }),
        )
        .await;
        assert_eq!(result.unwrap_err().0, StatusCode::FORBIDDEN);
    }
}
