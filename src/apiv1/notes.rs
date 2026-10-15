use super::{authenticate_active_bearer, bad_request, internal_error};
use crate::db::{
    notes::{self, Snapshot, Update},
    AppState,
};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
pub(super) async fn download(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Snapshot>, (StatusCode, String)> {
    authenticate_active_bearer(&state.pool, &headers).await?;
    Ok(Json(
        notes::current(&state.pool).await.map_err(internal_error)?,
    ))
}
pub(super) async fn upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(update): Json<Update>,
) -> Result<Json<Snapshot>, (StatusCode, String)> {
    let device = authenticate_active_bearer(&state.pool, &headers).await?;
    if update.base_revision < 0
        || update.operation_id.is_empty()
        || update.operation_id.len() > 128
        || update.text.len() > 32 * 1024 * 1024
    {
        return Err(bad_request("invalid notes update"));
    }
    let revision = notes::current(&state.pool)
        .await
        .map_err(internal_error)?
        .revision;
    if update.base_revision > revision {
        return Err(bad_request("notes revision belongs to a different server"));
    }
    Ok(Json(
        notes::update(&state.pool, &device.uuid, update)
            .await
            .map_err(internal_error)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{
        check_state,
        device::{insert_device, Device},
    };
    fn headers(token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("Authorization", format!("Bearer {token}").parse().unwrap());
        h
    }
    #[tokio::test]
    async fn notes_are_shared_only_with_approved_devices_and_validate_revisions() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        check_state(&pool).await.unwrap();
        insert_device(
            &pool,
            Device::new("desktop".into(), &"desktop-token".into()).unwrap(),
        )
        .await
        .unwrap();
        insert_device(
            &pool,
            Device::new("phone".into(), &"phone-token".into()).unwrap(),
        )
        .await
        .unwrap();
        let state = AppState { pool };
        assert_eq!(
            download(State(state.clone()), headers("invalid"))
                .await
                .unwrap_err()
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            download(State(state.clone()), headers("phone-token"))
                .await
                .unwrap_err()
                .0,
            StatusCode::FORBIDDEN
        );
        sqlx::query("UPDATE devices SET is_active=1")
            .execute(&state.pool)
            .await
            .unwrap();
        let _ = upload(
            State(state.clone()),
            headers("desktop-token"),
            Json(Update {
                operation_id: "a".into(),
                base_revision: 0,
                text: "desktop".into(),
            }),
        )
        .await
        .unwrap();
        let phone = download(State(state.clone()), headers("phone-token"))
            .await
            .unwrap()
            .0;
        assert_eq!(phone.text, "desktop");
        let merged = upload(
            State(state.clone()),
            headers("phone-token"),
            Json(Update {
                operation_id: "b".into(),
                base_revision: 0,
                text: "phone".into(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(merged.text.contains("desktop") && merged.text.contains("phone"));
        let invalid = upload(
            State(state.clone()),
            headers("phone-token"),
            Json(Update {
                operation_id: "c".into(),
                base_revision: 999,
                text: "bad".into(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(invalid.0, StatusCode::BAD_REQUEST);
        assert_eq!(
            download(State(state), headers("desktop-token"))
                .await
                .unwrap()
                .0
                .revision,
            merged.revision
        );
    }
}
