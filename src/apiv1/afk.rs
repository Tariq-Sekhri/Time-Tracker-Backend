use super::{authenticate_active_bearer, bad_request, internal_error};
use crate::db::{
    AppState,
    afk::{self, Evidence, PAGE_SIZE},
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;

pub(super) async fn upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(evidence): Json<Vec<Evidence>>,
) -> Result<StatusCode, (StatusCode, String)> {
    let device = authenticate_active_bearer(&state.pool, &headers).await?;
    if evidence.len() > PAGE_SIZE || evidence.iter().any(|e| !e.valid()) {
        return Err(bad_request("invalid AFK evidence or batch size"));
    }
    afk::upload(&state.pool, &device.uuid, &evidence)
        .await
        .map_err(internal_error)?;
    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
pub(super) struct Cursor {
    #[serde(default)]
    after: i64,
}

pub(super) async fn download(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(device): Path<String>,
    Query(cursor): Query<Cursor>,
) -> Result<Json<Vec<Evidence>>, (StatusCode, String)> {
    authenticate_active_bearer(&state.pool, &headers).await?;
    if cursor.after < 0 {
        return Err(bad_request("invalid AFK cursor"));
    }
    let active: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM devices WHERE uuid=? AND is_active=1)")
            .bind(&device)
            .fetch_one(&state.pool)
            .await
            .map_err(internal_error)?;
    if !active {
        return Err((StatusCode::NOT_FOUND, "device not found".into()));
    }
    Ok(Json(
        afk::download(&state.pool, &device, cursor.after)
            .await
            .map_err(internal_error)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{
        check_state,
        device::{Device, insert_device},
    };
    use sqlx::sqlite::SqlitePoolOptions;

    async fn state() -> (AppState, String, String) {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        check_state(&pool).await.unwrap();
        let device = Device::new("desktop".into(), &"afk-test".into()).unwrap();
        let uuid = device.uuid.clone();
        insert_device(&pool, device).await.unwrap();
        sqlx::query("UPDATE devices SET is_active=1")
            .execute(&pool)
            .await
            .unwrap();
        (AppState { pool }, "afk-test".into(), uuid)
    }
    fn headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", format!("Bearer {token}").parse().unwrap());
        headers
    }
    fn evidence(id: i64) -> Evidence {
        Evidence {
            id,
            start_time: 100,
            end_time: 200,
            state: 2,
            known_mask: 1,
            active_mask: 0,
            settings_json: "{}".into(),
            revision: 0,
        }
    }

    #[tokio::test]
    async fn afk_tail_updates_replay_and_device_separation() {
        let (state, token, uuid) = state().await;
        let other = Device::new("phone".into(), &"other-token".into()).unwrap();
        let other_uuid = other.uuid.clone();
        insert_device(&state.pool, other).await.unwrap();
        sqlx::query("UPDATE devices SET is_active=1")
            .execute(&state.pool)
            .await
            .unwrap();
        for _ in 0..2 {
            upload(
                State(state.clone()),
                headers(&token),
                Json(vec![evidence(1)]),
            )
            .await
            .unwrap();
        }
        upload(
            State(state.clone()),
            headers("other-token"),
            Json(vec![evidence(1)]),
        )
        .await
        .unwrap();
        let first = download(
            State(state.clone()),
            headers(&token),
            Path(uuid.clone()),
            Query(Cursor { after: 0 }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(first.len(), 1);
        let revision = first[0].revision;
        let mut extended = evidence(1);
        extended.end_time = 300;
        upload(State(state.clone()), headers(&token), Json(vec![extended]))
            .await
            .unwrap();
        // A late retry of the older sample cannot truncate ongoing evidence.
        upload(
            State(state.clone()),
            headers(&token),
            Json(vec![evidence(1)]),
        )
        .await
        .unwrap();
        let next = download(
            State(state.clone()),
            headers(&token),
            Path(uuid.clone()),
            Query(Cursor { after: revision }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].end_time, 300);
        let other = afk::download(&state.pool, &other_uuid, 0).await.unwrap();
        assert_eq!(other.len(), 1);
        assert_eq!(other[0].end_time, 200);
        let counts: (i64, i64) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM afk_intervals),(SELECT COUNT(*) FROM afk_configurations)",
        )
        .fetch_one(&state.pool)
        .await
        .unwrap();
        assert_eq!(counts, (2, 1));
        sqlx::query("DELETE FROM devices WHERE uuid=?")
            .bind(&uuid)
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(
            afk::download(&state.pool, &uuid, 0)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn afk_requires_approval_and_rejects_invalid_batch_atomically() {
        let (state, token, uuid) = state().await;
        assert_eq!(
            upload(
                State(state.clone()),
                headers("invalid"),
                Json(vec![evidence(1)])
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            download(
                State(state.clone()),
                HeaderMap::new(),
                Path(uuid.clone()),
                Query(Cursor { after: 0 })
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::UNAUTHORIZED
        );
        let mut invalid = evidence(2);
        invalid.end_time = invalid.start_time;
        assert_eq!(
            upload(
                State(state.clone()),
                headers(&token),
                Json(vec![evidence(1), invalid])
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::BAD_REQUEST
        );
        assert!(
            afk::download(&state.pool, &uuid, 0)
                .await
                .unwrap()
                .is_empty()
        );
        sqlx::query("UPDATE devices SET is_active=0")
            .execute(&state.pool)
            .await
            .unwrap();
        assert_eq!(
            upload(
                State(state.clone()),
                headers(&token),
                Json(vec![evidence(1)])
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            download(
                State(state),
                headers(&token),
                Path(uuid),
                Query(Cursor { after: 0 })
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn afk_revision_pagination_delivers_all_history_and_changed_tail() {
        let (state, token, uuid) = state().await;
        let rows: Vec<_> = (1..=PAGE_SIZE as i64).map(evidence).collect();
        upload(State(state.clone()), headers(&token), Json(rows))
            .await
            .unwrap();
        upload(
            State(state.clone()),
            headers(&token),
            Json(vec![evidence(PAGE_SIZE as i64 + 1)]),
        )
        .await
        .unwrap();
        let first = afk::download(&state.pool, &uuid, 0).await.unwrap();
        assert_eq!(first.len(), PAGE_SIZE);
        let next = afk::download(&state.pool, &uuid, first.last().unwrap().revision)
            .await
            .unwrap();
        assert_eq!(next.len(), 1);
        let cursor = next[0].revision;
        let mut changed = evidence(1);
        changed.end_time = 400;
        upload(State(state.clone()), headers(&token), Json(vec![changed]))
            .await
            .unwrap();
        let next = afk::download(&state.pool, &uuid, cursor).await.unwrap();
        assert_eq!(next.len(), 1);
        assert_eq!((next[0].id, next[0].end_time), (1, 400));
    }
}
