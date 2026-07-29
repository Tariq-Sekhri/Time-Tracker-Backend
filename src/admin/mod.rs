use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use constant_time_eq::constant_time_eq;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::db::device::{generate_auth_token, PubDevice};
use crate::db::log::Log;
use crate::db::AppState;
use crate::{error, info};

const LOG_LIST_LIMIT: i64 = 2000;
const ADMIN_COOKIE_NAME: &str = "time_tracker_admin";

#[derive(Clone)]
struct AdminAuthState {
    password_hash: [u8; 32],
    session_token: String,
}

pub fn router(state: AppState, admin_password: String) -> Router {
    let auth_state = AdminAuthState {
        password_hash: Sha256::digest(admin_password.as_bytes()).into(),
        session_token: generate_auth_token().expect("OS randomness must be available"),
    };

    let protected = Router::new()
        .route("/", get(admin_index))
        .route("/devices", get(list_devices))
        .route("/devices/{uuid}", get(edit_device_page).post(update_device))
        .route("/devices/{uuid}/activate", post(activate_device))
        .route("/devices/{uuid}/deactivate", post(deactivate_device))
        .route("/devices/{uuid}/delete", post(delete_device))
        .route("/logs", get(list_logs))
        .route("/logs/{device_uuid}/{id}", get(edit_log_page).post(update_log))
        .route("/logs/{device_uuid}/{id}/delete", post(delete_log))
        .route("/logout", post(logout))
        .with_state(state)
        .layer(middleware::from_fn_with_state(
            auth_state.clone(),
            require_admin,
        ));

    Router::new()
        .route("/login", get(login_page).post(login))
        .with_state(auth_state)
        .merge(protected)
}

async fn require_admin(
    State(auth): State<AdminAuthState>,
    request: Request,
    next: Next,
) -> Response {
    if has_valid_admin_cookie(request.headers(), &auth.session_token) {
        next.run(request).await
    } else {
        Redirect::to("/admin/login").into_response()
    }
}

fn has_valid_admin_cookie(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|cookie| {
                let (name, value) = cookie.trim().split_once('=')?;
                (name == ADMIN_COOKIE_NAME).then_some(value)
            })
        })
        .map(|value| {
            value.len() == expected.len() && constant_time_eq(value.as_bytes(), expected.as_bytes())
        })
        .unwrap_or(false)
}

async fn login_page() -> Html<String> {
    Html(login_html(None))
}

#[derive(Deserialize)]
struct LoginForm {
    password: String,
}

async fn login(
    State(auth): State<AdminAuthState>,
    Form(form): Form<LoginForm>,
) -> Response {
    let supplied_hash: [u8; 32] = Sha256::digest(form.password.as_bytes()).into();
    if !constant_time_eq(&supplied_hash, &auth.password_hash) {
        return (StatusCode::UNAUTHORIZED, Html(login_html(Some("Incorrect password"))))
            .into_response();
    }

    let mut response = Redirect::to("/admin/devices").into_response();
    let cookie = format!(
        "{ADMIN_COOKIE_NAME}={}; Path=/admin; HttpOnly; SameSite=Strict",
        auth.session_token
    );
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("generated session cookie is valid"),
    );
    response
}

async fn logout() -> Response {
    let mut response = Redirect::to("/admin/login").into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "time_tracker_admin=; Path=/admin; HttpOnly; SameSite=Strict; Max-Age=0",
        ),
    );
    response
}

async fn admin_index() -> impl IntoResponse {
    Redirect::to("/admin/devices")
}

async fn list_devices(State(state): State<AppState>) -> Result<Html<String>, (StatusCode, String)> {
    let devices: Vec<PubDevice> = sqlx::query_as(
        "SELECT uuid, name, last_sync_id, is_active
         FROM devices ORDER BY is_active ASC, name ASC"
    )
        .fetch_all(&state.pool)
        .await
        .map_err(internal_error)?;

    let mut rows = String::new();
    for d in &devices {
        let status = if d.is_active {
            "<span class=\"status active\">Active</span>"
        } else {
            "<span class=\"status pending\">Waiting for approval</span>"
        };
        let action = if d.is_active {
            format!(
                "<form method=\"post\" action=\"/admin/devices/{}/deactivate\"><button class=\"secondary\" type=\"submit\">Deactivate</button></form>",
                esc(&d.uuid)
            )
        } else {
            format!(
                "<form method=\"post\" action=\"/admin/devices/{}/activate\"><button type=\"submit\">Activate</button></form>",
                esc(&d.uuid)
            )
        };
        rows.push_str(&format!(
            "<tr><td><strong>{}</strong><div class=\"uuid\">{}</div></td><td>{}</td><td>{}</td><td class=\"actions\">{}<a href=\"/admin/devices/{}\">Manage</a></td></tr>",
            esc(&d.name),
            esc(&d.uuid),
            status,
            d.last_sync_id,
            action,
            esc(&d.uuid),
        ));
    }

    let pending_count = devices.iter().filter(|device| !device.is_active).count();

    Ok(Html(page(
        "Devices",
        &format!(
            "<div class=\"title-row\"><div><h1>Device approvals</h1><p>{} waiting · {} total</p></div></div>\
            <table>\
            <tr><th>Device</th><th>Status</th><th>Last sync ID</th><th>Actions</th></tr>\
            {rows}\
            </table>",
            pending_count,
            devices.len(),
        ),
    )))
}

async fn edit_device_page(
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Result<Html<String>, (StatusCode, String)> {
    let device: PubDevice = sqlx::query_as(
        "SELECT uuid, name, last_sync_id, is_active FROM devices WHERE uuid = ?",
    )
    .bind(&uuid)
    .fetch_optional(&state.pool)
    .await
    .map_err(internal_error)?
    .ok_or_else(|| not_found("device not found"))?;

    Ok(Html(page(
        "Edit device",
        &format!(
            "<h1>Manage device</h1>\
            <p>Status: {}</p>\
            <form method=\"post\" action=\"/admin/devices/{}/{}\">\
            <button type=\"submit\">{}</button>\
            </form>\
            <form method=\"post\" action=\"/admin/devices/{}\">\
            <p>Name: <input name=\"name\" value=\"{}\" size=\"40\"></p>\
            <p>last_sync_id: <input name=\"last_sync_id\" value=\"{}\" size=\"20\"></p>\
            <p><button type=\"submit\">Save</button></p>\
            </form>\
            <form method=\"post\" action=\"/admin/devices/{}/delete\" onsubmit=\"return confirm('Delete device and all its logs?');\">\
            <button type=\"submit\">Delete device</button>\
            </form>\
            <p><a href=\"/admin/logs?device_uuid={}\">View logs for this device</a></p>",
            if device.is_active { "Active" } else { "Waiting for approval" },
            esc(&device.uuid),
            if device.is_active { "deactivate" } else { "activate" },
            if device.is_active { "Deactivate" } else { "Activate" },
            esc(&device.uuid),
            esc(&device.name),
            device.last_sync_id,
            esc(&device.uuid),
            esc(&device.uuid),
        ),
    )))
}

async fn activate_device(
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Result<Redirect, (StatusCode, String)> {
    set_device_active(&state, &uuid, true).await?;
    info!("admin activated device uuid={}", uuid);
    Ok(Redirect::to("/admin/devices"))
}

async fn deactivate_device(
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Result<Redirect, (StatusCode, String)> {
    set_device_active(&state, &uuid, false).await?;
    info!("admin deactivated device uuid={}", uuid);
    Ok(Redirect::to("/admin/devices"))
}

async fn set_device_active(
    state: &AppState,
    uuid: &str,
    is_active: bool,
) -> Result<(), (StatusCode, String)> {
    let result = sqlx::query("UPDATE devices SET is_active = ? WHERE uuid = ?")
        .bind(is_active)
        .bind(uuid)
        .execute(&state.pool)
        .await
        .map_err(internal_error)?;
    if result.rows_affected() == 0 {
        return Err(not_found("device not found"));
    }
    Ok(())
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
    let mut tx = state.pool.begin().await.map_err(internal_error)?;
    sqlx::query("DELETE FROM logs WHERE device_uuid = ?")
        .bind(&uuid)
        .execute(&mut *tx)
        .await
        .map_err(internal_error)?;
    let result = sqlx::query("DELETE FROM devices WHERE uuid = ?")
        .bind(&uuid)
        .execute(&mut *tx)
        .await
        .map_err(internal_error)?;

    if result.rows_affected() == 0 {
        return Err(not_found("device not found"));
    }
    tx.commit().await.map_err(internal_error)?;

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
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{title}</title>{style}</head><body>\
        <header><nav><strong>Time Tracker Admin</strong><span><a href=\"/admin/devices\">Devices</a><a href=\"/admin/logs\">Logs</a><form method=\"post\" action=\"/admin/logout\"><button class=\"link\" type=\"submit\">Log out</button></form></span></nav></header>\
        <main>{body}</main>\
        </body></html>",
        title = esc(title),
        style = shared_style(),
    )
}

fn login_html(error: Option<&str>) -> String {
    let error = error
        .map(|message| format!("<p class=\"error\">{}</p>", esc(message)))
        .unwrap_or_default();
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Admin login</title>{}</head><body class=\"login\"><main class=\"login-card\">\
        <h1>Time Tracker Admin</h1><p>Sign in to approve registered devices.</p>{error}\
        <form method=\"post\" action=\"/admin/login\"><label>Password<input name=\"password\" type=\"password\" autocomplete=\"current-password\" required autofocus></label><button type=\"submit\">Log in</button></form>\
        </main></body></html>",
        shared_style(),
    )
}

fn shared_style() -> &'static str {
    r#"<style>
    :root{color-scheme:dark;font-family:Inter,ui-sans-serif,system-ui,sans-serif;background:#0b1120;color:#e5e7eb}
    *{box-sizing:border-box}body{margin:0;background:#0b1120;color:#e5e7eb}header{border-bottom:1px solid #263244;background:#111827}
    nav{max-width:1100px;margin:auto;padding:16px 24px;display:flex;align-items:center;justify-content:space-between}nav span{display:flex;align-items:center;gap:18px}nav form,.actions form{display:inline;margin:0}
    a,.link{color:#93c5fd;text-decoration:none}.link{border:0;background:none;padding:0;font:inherit;cursor:pointer}main{max-width:1100px;margin:36px auto;padding:0 24px}
    h1{margin:0 0 8px;font-size:28px}p{color:#9ca3af}.title-row{display:flex;justify-content:space-between;align-items:center;margin-bottom:24px}
    table{width:100%;border-collapse:collapse;background:#111827;border:1px solid #263244;border-radius:10px;overflow:hidden}th,td{padding:14px 16px;text-align:left;border-bottom:1px solid #263244}th{font-size:12px;text-transform:uppercase;letter-spacing:.06em;color:#9ca3af}tr:last-child td{border-bottom:0}
    .uuid{font-family:ui-monospace,monospace;color:#6b7280;font-size:12px;margin-top:4px}.status{display:inline-block;padding:4px 9px;border-radius:999px;font-size:12px;font-weight:700}.active{background:#064e3b;color:#a7f3d0}.pending{background:#78350f;color:#fde68a}
    .actions{display:flex;align-items:center;gap:12px}button{border:0;border-radius:7px;background:#2563eb;color:white;padding:9px 14px;font-weight:700;cursor:pointer}.secondary{background:#374151}button:hover{filter:brightness(1.12)}
    input{display:block;width:100%;margin-top:7px;padding:10px 12px;border:1px solid #374151;border-radius:7px;background:#0f172a;color:white}label{display:block;margin:18px 0;color:#d1d5db}
    .login{min-height:100vh;display:grid;place-items:center}.login main{margin:0}.login-card{width:min(420px,calc(100vw - 32px));padding:30px;background:#111827;border:1px solid #263244;border-radius:12px}.login-card button{width:100%;margin-top:4px}.error{color:#fca5a5;background:#450a0a;padding:10px 12px;border-radius:7px}
    </style>"#
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{check_state, device::Device};
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn deleting_a_device_removes_its_logs_in_the_same_operation() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        check_state(&pool).await.unwrap();

        let token = "test-token".to_string();
        let device = Device::new("desktop".to_string(), &token).unwrap();
        let uuid = device.uuid.clone();
        crate::db::device::insert_device(&pool, device).await.unwrap();
        sqlx::query(
            "INSERT INTO logs (id, device_uuid, app, timestamp, duration)
             VALUES (1, ?, 'Editor', 100, 15)",
        )
        .bind(&uuid)
        .execute(&pool)
        .await
        .unwrap();

        let _redirect =
            delete_device(State(AppState { pool: pool.clone() }), Path(uuid.clone()))
                .await
                .unwrap();

        let device_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM devices WHERE uuid = ?")
                .bind(&uuid)
                .fetch_one(&pool)
                .await
                .unwrap();
        let log_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM logs WHERE device_uuid = ?")
            .bind(uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!((device_count, log_count), (0, 0));
    }

    #[tokio::test]
    async fn admin_can_activate_a_pending_device() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        check_state(&pool).await.unwrap();
        let token = "test-token".to_string();
        let device = Device::new("desktop".to_string(), &token).unwrap();
        let uuid = device.uuid.clone();
        crate::db::device::insert_device(&pool, device).await.unwrap();
        let state = AppState { pool };

        set_device_active(&state, &uuid, true).await.unwrap();

        let active: bool = sqlx::query_scalar("SELECT is_active FROM devices WHERE uuid = ?")
            .bind(uuid)
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert!(active);
    }

    #[test]
    fn admin_cookie_must_match_the_generated_session() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("other=x; time_tracker_admin=correct"),
        );
        assert!(has_valid_admin_cookie(&headers, "correct"));
        assert!(!has_valid_admin_cookie(&headers, "incorrect"));
    }
}
