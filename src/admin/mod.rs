use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::{Form, Router};
use serde::Deserialize;

use crate::db::device::PubDevice;
use crate::db::log::Log;
use crate::db::AppState;
use crate::{error, info};

const LOG_LIST_LIMIT: i64 = 2000;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(admin_index))
        .route("/devices", get(list_devices))
        .route("/devices/{uuid}", get(edit_device_page).post(update_device))
        .route("/devices/{uuid}/delete", post(delete_device))
        .route("/logs", get(list_logs))
        .route("/logs/{device_uuid}/{id}", get(edit_log_page).post(update_log))
        .route("/logs/{device_uuid}/{id}/delete", post(delete_log))
        .with_state(state)
}

async fn admin_index() -> impl IntoResponse {
    Redirect::to("/admin/devices")
}

async fn list_devices(State(state): State<AppState>) -> Result<Html<String>, (StatusCode, String)> {
    let devices: Vec<PubDevice> = sqlx::query_as("SELECT uuid, name, last_sync_id FROM devices ORDER BY name")
        .fetch_all(&state.pool)
        .await
        .map_err(internal_error)?;

    let mut rows = String::new();
    for d in &devices {
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td><a href=\"/admin/devices/{}\">edit</a></td></tr>",
            esc(&d.uuid),
            esc(&d.name),
            d.last_sync_id,
            esc(&d.uuid),
        ));
    }

    Ok(Html(page(
        "Devices",
        &format!(
            "<h1>Devices</h1><p>{} device(s)</p>\
            <table border=\"1\" cellpadding=\"4\">\
            <tr><th>UUID</th><th>Name</th><th>last_sync_id</th><th></th></tr>\
            {rows}\
            </table>",
            devices.len()
        ),
    )))
}

async fn edit_device_page(
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Result<Html<String>, (StatusCode, String)> {
    let device: PubDevice = sqlx::query_as(
        "SELECT uuid, name, last_sync_id FROM devices WHERE uuid = ?",
    )
    .bind(&uuid)
    .fetch_optional(&state.pool)
    .await
    .map_err(internal_error)?
    .ok_or_else(|| not_found("device not found"))?;

    Ok(Html(page(
        "Edit device",
        &format!(
            "<h1>Edit device</h1>\
            <form method=\"post\" action=\"/admin/devices/{}\">\
            <p>Name: <input name=\"name\" value=\"{}\" size=\"40\"></p>\
            <p>last_sync_id: <input name=\"last_sync_id\" value=\"{}\" size=\"20\"></p>\
            <p><button type=\"submit\">Save</button></p>\
            </form>\
            <form method=\"post\" action=\"/admin/devices/{}/delete\" onsubmit=\"return confirm('Delete device and all its logs?');\">\
            <button type=\"submit\">Delete device</button>\
            </form>\
            <p><a href=\"/admin/logs?device_uuid={}\">View logs for this device</a></p>",
            esc(&device.uuid),
            esc(&device.name),
            device.last_sync_id,
            esc(&device.uuid),
            esc(&device.uuid),
        ),
    )))
}

#[derive(Deserialize)]
struct DeviceForm {
    name: String,
    last_sync_id: String,
}

async fn update_device(
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    Form(form): Form<DeviceForm>,
) -> Result<Redirect, (StatusCode, String)> {
    let last_sync_id: i64 = form
        .last_sync_id
        .trim()
        .parse()
        .map_err(|_| bad_request("last_sync_id must be a number"))?;

    let result = sqlx::query("UPDATE devices SET name = ?, last_sync_id = ? WHERE uuid = ?")
        .bind(form.name.trim())
        .bind(last_sync_id)
        .bind(&uuid)
        .execute(&state.pool)
        .await
        .map_err(internal_error)?;

    if result.rows_affected() == 0 {
        return Err(not_found("device not found"));
    }

    info!("admin updated device uuid={}", uuid);
    Ok(Redirect::to(&format!("/admin/devices/{}", uuid)))
}

async fn delete_device(
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Result<Redirect, (StatusCode, String)> {
    let result = sqlx::query("DELETE FROM devices WHERE uuid = ?")
        .bind(&uuid)
        .execute(&state.pool)
        .await
        .map_err(internal_error)?;

    if result.rows_affected() == 0 {
        return Err(not_found("device not found"));
    }

    info!("admin deleted device uuid={}", uuid);
    Ok(Redirect::to("/admin/devices"))
}

#[derive(Deserialize)]
struct LogsQuery {
    device_uuid: Option<String>,
}

async fn list_logs(
    State(state): State<AppState>,
    Query(query): Query<LogsQuery>,
) -> Result<Html<String>, (StatusCode, String)> {
    let logs: Vec<Log> = if let Some(device_uuid) = query.device_uuid.as_ref().filter(|s| !s.is_empty())
    {
        sqlx::query_as(
            "SELECT id, device_uuid, app, timestamp, duration FROM logs \
             WHERE device_uuid = ? ORDER BY timestamp DESC LIMIT ?",
        )
        .bind(device_uuid)
        .bind(LOG_LIST_LIMIT)
        .fetch_all(&state.pool)
        .await
        .map_err(internal_error)?
    } else {
        sqlx::query_as(
            "SELECT id, device_uuid, app, timestamp, duration FROM logs \
             ORDER BY timestamp DESC LIMIT ?",
        )
        .bind(LOG_LIST_LIMIT)
        .fetch_all(&state.pool)
        .await
        .map_err(internal_error)?
    };

    let filter_note = query
        .device_uuid
        .as_ref()
        .filter(|s| !s.is_empty())
        .map(|u| format!("<p>Filtered by device: {}</p>", esc(u)))
        .unwrap_or_default();

    let mut rows = String::new();
    for log in &logs {
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td>\
            <td><a href=\"/admin/logs/{}/{}\">edit</a></td></tr>",
            log.id,
            esc(&log.device_uuid),
            esc(&log.app),
            log.timestamp,
            log.duration,
            esc(&log.device_uuid),
            log.id,
        ));
    }

    Ok(Html(page(
        "Logs",
        &format!(
            "<h1>Logs</h1>\
            {filter_note}\
            <p>Showing up to {LOG_LIST_LIMIT} rows (newest first).</p>\
            <p>Filter: <form method=\"get\" action=\"/admin/logs\">\
            device_uuid <input name=\"device_uuid\" value=\"{}\" size=\"40\">\
            <button type=\"submit\">Filter</button>\
            <a href=\"/admin/logs\">clear</a></form></p>\
            <table border=\"1\" cellpadding=\"4\">\
            <tr><th>id</th><th>device_uuid</th><th>app</th><th>timestamp</th><th>duration</th><th></th></tr>\
            {rows}\
            </table>",
            esc(query.device_uuid.as_deref().unwrap_or("")),
        ),
    )))
}

async fn edit_log_page(
    State(state): State<AppState>,
    Path((device_uuid, id)): Path<(String, i64)>,
) -> Result<Html<String>, (StatusCode, String)> {
    let log: Log = sqlx::query_as(
        "SELECT id, device_uuid, app, timestamp, duration FROM logs WHERE device_uuid = ? AND id = ?",
    )
    .bind(&device_uuid)
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(internal_error)?
    .ok_or_else(|| not_found("log not found"))?;

    Ok(Html(page(
        "Edit log",
        &format!(
            "<h1>Edit log</h1>\
            <p>device_uuid: {}</p><p>id: {}</p>\
            <form method=\"post\" action=\"/admin/logs/{}/{}\">\
            <p>app: <input name=\"app\" value=\"{}\" size=\"60\"></p>\
            <p>timestamp: <input name=\"timestamp\" value=\"{}\" size=\"20\"></p>\
            <p>duration: <input name=\"duration\" value=\"{}\" size=\"20\"></p>\
            <p><button type=\"submit\">Save</button></p>\
            </form>\
            <form method=\"post\" action=\"/admin/logs/{}/{}/delete\" onsubmit=\"return confirm('Delete this log?');\">\
            <button type=\"submit\">Delete log</button>\
            </form>",
            esc(&log.device_uuid),
            log.id,
            esc(&log.device_uuid),
            log.id,
            esc(&log.app),
            log.timestamp,
            log.duration,
            esc(&log.device_uuid),
            log.id,
        ),
    )))
}

#[derive(Deserialize)]
struct LogForm {
    app: String,
    timestamp: String,
    duration: String,
}

async fn update_log(
    State(state): State<AppState>,
    Path((device_uuid, id)): Path<(String, i64)>,
    Form(form): Form<LogForm>,
) -> Result<Redirect, (StatusCode, String)> {
    let timestamp: i64 = form
        .timestamp
        .trim()
        .parse()
        .map_err(|_| bad_request("timestamp must be a number"))?;
    let duration: i64 = form
        .duration
        .trim()
        .parse()
        .map_err(|_| bad_request("duration must be a number"))?;

    let result = sqlx::query(
        "UPDATE logs SET app = ?, timestamp = ?, duration = ? WHERE device_uuid = ? AND id = ?",
    )
    .bind(form.app.trim())
    .bind(timestamp)
    .bind(duration)
    .bind(&device_uuid)
    .bind(id)
    .execute(&state.pool)
    .await
    .map_err(internal_error)?;

    if result.rows_affected() == 0 {
        return Err(not_found("log not found"));
    }

    info!("admin updated log device={} id={}", device_uuid, id);
    Ok(Redirect::to(&format!("/admin/logs/{}/{}", device_uuid, id)))
}

async fn delete_log(
    State(state): State<AppState>,
    Path((device_uuid, id)): Path<(String, i64)>,
) -> Result<Redirect, (StatusCode, String)> {
    let result = sqlx::query("DELETE FROM logs WHERE device_uuid = ? AND id = ?")
        .bind(&device_uuid)
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(internal_error)?;

    if result.rows_affected() == 0 {
        return Err(not_found("log not found"));
    }

    info!("admin deleted log device={} id={}", device_uuid, id);
    Ok(Redirect::to("/admin/logs"))
}

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title></head><body>\
        <p><a href=\"/admin/devices\">Devices</a> | <a href=\"/admin/logs\">Logs</a></p>\
        {body}\
        </body></html>",
        title = esc(title),
    )
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn internal_error<E: std::fmt::Display>(err: E) -> (StatusCode, String) {
    error!("admin internal error: {}", err);
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}

fn bad_request(message: &str) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, message.to_string())
}

fn not_found(message: &str) -> (StatusCode, String) {
    (StatusCode::NOT_FOUND, message.to_string())
}
