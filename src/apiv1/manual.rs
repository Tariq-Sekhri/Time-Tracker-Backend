use super::{authenticate_active_bearer, bad_request, internal_error};
use crate::db::{
    AppState,
    manual::{self, Entry, Project, PutTimer, Timer},
};
use crate::info;
use axum::{
    Json,
    extract::{
        Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;
use std::sync::LazyLock;
use tokio::sync::broadcast;

/// Clock skew tolerated between devices and the server, in seconds.
const MAX_FUTURE_SECS: i64 = 60;

static EVENTS: LazyLock<broadcast::Sender<String>> = LazyLock::new(|| broadcast::channel(64).0);

fn publish(event: serde_json::Value) {
    // No receivers just means no device is connected right now.
    let _ = EVENTS.send(event.to_string());
}

fn timer_event(timer: Option<&Timer>) -> serde_json::Value {
    json!({ "type": "timer", "timer": timer })
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub(super) async fn get_timer(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Option<Timer>>, (StatusCode, String)> {
    authenticate_active_bearer(&state.pool, &headers).await?;
    Ok(Json(manual::get_timer(&state.pool).await.map_err(internal_error)?))
}

pub(super) async fn put_timer(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(timer): Json<Timer>,
) -> Result<Response, (StatusCode, String)> {
    let device = authenticate_active_bearer(&state.pool, &headers).await?;
    if !manual::valid_session(&timer.session_id)
        || !manual::valid_text(&timer.title, timer.project_name.as_deref())
        || timer.start_time < 0
        || timer.start_time > now() + MAX_FUTURE_SECS
    {
        return Err(bad_request("invalid manual timer"));
    }
    match manual::put_timer(&state.pool, &device.uuid, &timer)
        .await
        .map_err(internal_error)?
    {
        Some(PutTimer::Saved(saved)) => {
            info!("manual timer saved device={} version={}", device.uuid, saved.version);
            publish(timer_event(Some(&saved)));
            Ok(Json(saved).into_response())
        }
        Some(PutTimer::Conflict(current)) => Ok((StatusCode::CONFLICT, Json(current)).into_response()),
        None => Err((StatusCode::GONE, "manual timer already finished".into())),
    }
}

pub(super) async fn finish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(entry): Json<Entry>,
) -> Result<Json<Entry>, (StatusCode, String)> {
    let device = authenticate_active_bearer(&state.pool, &headers).await?;
    if !manual::valid_session(&entry.session_id)
        || !manual::valid_text(&entry.title, entry.project_name.as_deref())
        || entry.start_time < 0
        || entry.end_time <= entry.start_time
        || entry.end_time > now() + MAX_FUTURE_SECS
    {
        return Err(bad_request("invalid manual entry"));
    }
    let (stored, cleared) = manual::finish(&state.pool, &device.uuid, &entry)
        .await
        .map_err(internal_error)?;
    info!("manual entry recorded device={} id={}", device.uuid, stored.id);
    if cleared {
        publish(timer_event(None));
    }
    publish(json!({ "type": "entry", "entry": stored }));
    Ok(Json(stored))
}

#[derive(Deserialize)]
pub(super) struct Cursor {
    #[serde(default)]
    after: i64,
}

pub(super) async fn entries(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(cursor): Query<Cursor>,
) -> Result<Json<Vec<Entry>>, (StatusCode, String)> {
    authenticate_active_bearer(&state.pool, &headers).await?;
    Ok(Json(
        manual::entries_after(&state.pool, cursor.after)
            .await
            .map_err(internal_error)?,
    ))
}

pub(super) async fn get_projects(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Project>>, (StatusCode, String)> {
    authenticate_active_bearer(&state.pool, &headers).await?;
    Ok(Json(manual::get_projects(&state.pool).await.map_err(internal_error)?))
}

pub(super) async fn put_projects(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(projects): Json<Vec<Project>>,
) -> Result<StatusCode, (StatusCode, String)> {
    authenticate_active_bearer(&state.pool, &headers).await?;
    if projects.len() > 1000
        || projects.iter().any(|p| {
            p.name.trim().is_empty() || p.name.chars().count() > 100 || !manual::valid_color(&p.color)
        })
    {
        return Err(bad_request("invalid manual projects"));
    }
    if manual::put_projects(&state.pool, &projects)
        .await
        .map_err(internal_error)?
    {
        publish(json!({ "type": "projects" }));
    }
    Ok(StatusCode::OK)
}

/// Pushes manual timer changes to connected devices. The current timer is sent
/// on connect so a reconnecting device never relies on missed events.
pub(super) async fn websocket(
    State(state): State<AppState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, (StatusCode, String)> {
    let device = authenticate_active_bearer(&state.pool, &headers).await?;
    info!("manual websocket connected device={}", device.uuid);
    Ok(upgrade.on_upgrade(move |socket| stream(socket, state)))
}

async fn stream(mut socket: WebSocket, state: AppState) {
    let mut events = EVENTS.subscribe();
    let mut send_current = true;
    loop {
        if std::mem::take(&mut send_current) {
            let Ok(timer) = manual::get_timer(&state.pool).await else { return };
            if socket
                .send(Message::Text(timer_event(timer.as_ref()).to_string().into()))
                .await
                .is_err()
            {
                return;
            }
        }
        tokio::select! {
            event = events.recv() => match event {
                Ok(text) => if socket.send(Message::Text(text.into())).await.is_err() { return },
                // A slow client missed events; the current timer replaces them.
                Err(broadcast::error::RecvError::Lagged(_)) => send_current = true,
                Err(broadcast::error::RecvError::Closed) => return,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                Some(Ok(_)) => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{
        check_state,
        device::{Device, insert_device},
    };
    use sqlx::sqlite::SqlitePoolOptions;

    async fn state() -> (AppState, String) {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        check_state(&pool).await.unwrap();
        for (name, token) in [("desktop", "desktop-token"), ("phone", "phone-token")] {
            insert_device(&pool, Device::new(name.into(), &token.into()).unwrap())
                .await
                .unwrap();
        }
        sqlx::query("UPDATE devices SET is_active=1")
            .execute(&pool)
            .await
            .unwrap();
        (AppState { pool }, "desktop-token".into())
    }

    fn headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", format!("Bearer {token}").parse().unwrap());
        headers
    }

    fn timer(session: &str, title: &str) -> Timer {
        Timer {
            session_id: session.into(),
            title: title.into(),
            project_name: Some("Client".into()),
            start_time: 100,
            version: 0,
            device_uuid: String::new(),
        }
    }

    fn entry(session: &str) -> Entry {
        Entry {
            id: 0,
            session_id: session.into(),
            title: "Planning".into(),
            project_name: None,
            start_time: 100,
            end_time: 200,
            device_uuid: String::new(),
        }
    }

    #[tokio::test]
    async fn timer_started_on_one_device_is_stopped_from_another() {
        let (state, token) = state().await;
        let mut events = EVENTS.subscribe();
        let response = put_timer(State(state.clone()), headers(&token), Json(timer("a", "Plan")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        // Other tests share the event channel, so look for this timer's event.
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !events.recv().await.unwrap().contains("\"Plan\"") {}
        })
        .await
        .unwrap();

        let response = put_timer(State(state.clone()), headers("phone-token"), Json(timer("a", "Planning")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let current = get_timer(State(state.clone()), headers("phone-token")).await.unwrap().0.unwrap();
        assert_eq!((current.title.as_str(), current.version), ("Planning", 2));

        // A second device cannot start a parallel timer; it gets the running one.
        let response = put_timer(State(state.clone()), headers("phone-token"), Json(timer("b", "Other")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);

        for _ in 0..2 {
            let stored = finish(State(state.clone()), headers("phone-token"), Json(entry("a")))
                .await
                .unwrap()
                .0;
            assert_eq!(stored.id, 1);
        }
        assert!(get_timer(State(state.clone()), headers(&token)).await.unwrap().0.is_none());
        let pulled = entries(State(state.clone()), headers(&token), Query(Cursor { after: 0 }))
            .await
            .unwrap()
            .0;
        assert_eq!(pulled.len(), 1);

        // A late retry of the finished session cannot restart it.
        let error = put_timer(State(state.clone()), headers(&token), Json(timer("a", "Plan")))
            .await
            .unwrap_err();
        assert_eq!(error.0, StatusCode::GONE);
        assert!(
            entries(State(state), headers(&token), Query(Cursor { after: 1 }))
                .await
                .unwrap()
                .0
                .is_empty()
        );
    }

    #[tokio::test]
    async fn finishing_an_offline_session_keeps_the_running_timer() {
        let (state, token) = state().await;
        put_timer(State(state.clone()), headers(&token), Json(timer("running", "Now")))
            .await
            .unwrap();
        let _ = finish(State(state.clone()), headers("phone-token"), Json(entry("offline")))
            .await
            .unwrap();
        let current = get_timer(State(state), headers(&token)).await.unwrap().0.unwrap();
        assert_eq!(current.session_id, "running");
    }

    #[tokio::test]
    async fn manual_sync_rejects_invalid_input_and_unapproved_devices() {
        let (state, token) = state().await;
        let mut bad = entry("a");
        bad.end_time = bad.start_time;
        assert_eq!(
            finish(State(state.clone()), headers(&token), Json(bad)).await.unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            put_timer(State(state.clone()), headers(&token), Json(timer("bad id!", "x")))
                .await
                .unwrap_err()
                .0,
            StatusCode::BAD_REQUEST
        );
        sqlx::query("UPDATE devices SET is_active=0")
            .execute(&state.pool)
            .await
            .unwrap();
        assert_eq!(
            get_timer(State(state), headers(&token)).await.unwrap_err().0,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn projects_merge_by_name() {
        let (state, token) = state().await;
        let project = |name: &str, color: &str| Project { name: name.into(), color: color.into() };
        put_projects(State(state.clone()), headers(&token), Json(vec![project("Client", "#AABBCC")]))
            .await
            .unwrap();
        put_projects(State(state.clone()), headers("phone-token"), Json(vec![project("client", "#112233"), project("Home", "#000000")]))
            .await
            .unwrap();
        let projects = get_projects(State(state.clone()), headers(&token)).await.unwrap().0;
        assert_eq!(projects, vec![project("client", "#112233"), project("Home", "#000000")]);
        assert_eq!(
            put_projects(State(state), headers(&token), Json(vec![project("x", "red")]))
                .await
                .unwrap_err()
                .0,
            StatusCode::BAD_REQUEST
        );
    }
}
