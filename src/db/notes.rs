use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

pub const PREFIX: &str = "<!--time-tracker-rich-notes-->";
#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct Snapshot {
    pub revision: i64,
    pub text: String,
}
#[derive(Deserialize)]
pub struct Update {
    pub operation_id: String,
    pub base_revision: i64,
    pub text: String,
}

pub async fn ensure_schema(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("CREATE TABLE IF NOT EXISTS notes_revisions (revision INTEGER PRIMARY KEY AUTOINCREMENT, text TEXT NOT NULL, device_uuid TEXT NOT NULL, operation_id TEXT NOT NULL, UNIQUE(device_uuid, operation_id))").execute(pool).await?;
    Ok(())
}
pub async fn current(pool: &SqlitePool) -> Result<Snapshot, sqlx::Error> {
    Ok(
        sqlx::query_as("SELECT revision,text FROM notes_revisions ORDER BY revision DESC LIMIT 1")
            .fetch_optional(pool)
            .await?
            .unwrap_or(Snapshot {
                revision: 0,
                text: String::new(),
            }),
    )
}
fn html(text: &str) -> String {
    text.strip_prefix(PREFIX)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!(
                "<p>{}</p>",
                text.replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
                    .replace('\n', "<br>")
            )
        })
}
fn merge(current: &str, incoming: &str) -> String {
    if current == incoming || incoming.is_empty() {
        return current.into();
    }
    if current.is_empty() {
        return incoming.into();
    }
    format!(
        "{PREFIX}{}<h3>Preserved conflicting edit</h3>{}",
        html(current),
        html(incoming)
    )
}
pub async fn update(
    pool: &SqlitePool,
    device: &str,
    update: Update,
) -> Result<Snapshot, sqlx::Error> {
    // Take SQLite's write lock before reading, so concurrent requests serialize.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let current: Snapshot =
        sqlx::query_as("SELECT revision,text FROM notes_revisions ORDER BY revision DESC LIMIT 1")
            .fetch_optional(&mut *tx)
            .await?
            .unwrap_or(Snapshot {
                revision: 0,
                text: String::new(),
            });
    let replay: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM notes_revisions WHERE device_uuid=? AND operation_id=?)",
    )
    .bind(device)
    .bind(&update.operation_id)
    .fetch_one(&mut *tx)
    .await?;
    if replay {
        tx.commit().await?;
        return Ok(current);
    }
    let text = if update.base_revision == current.revision {
        update.text
    } else {
        merge(&current.text, &update.text)
    };
    // Even a no-op is recorded: a lost response can be retried without merging twice.
    let revision =
        sqlx::query("INSERT INTO notes_revisions(text,device_uuid,operation_id) VALUES(?,?,?)")
            .bind(&text)
            .bind(device)
            .bind(update.operation_id)
            .execute(&mut *tx)
            .await?
            .last_insert_rowid();
    tx.commit().await?;
    Ok(Snapshot { revision, text })
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn pool() -> SqlitePool {
        let p = SqlitePool::connect("sqlite::memory:").await.unwrap();
        ensure_schema(&p).await.unwrap();
        p
    }
    async fn put(p: &SqlitePool, d: &str, id: &str, base: i64, text: &str) -> Snapshot {
        update(
            p,
            d,
            Update {
                operation_id: id.into(),
                base_revision: base,
                text: text.into(),
            },
        )
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn offline_conflicts_retries_and_clear_keep_content() {
        let p = pool().await;
        let a = put(&p, "desktop", "1", 0, "desktop notes").await;
        let b = put(&p, "phone", "1", 0, "phone notes").await;
        assert!(b.text.contains("desktop notes") && b.text.contains("phone notes"));
        let retry = put(&p, "phone", "1", 0, "phone notes").await;
        assert_eq!(retry.revision, b.revision);
        assert_eq!(retry.text, b.text);
        let stale_clear = put(&p, "desktop", "2", a.revision, "").await;
        assert_eq!(stale_clear.text, b.text);
        let clear = put(&p, "phone", "2", stale_clear.revision, "").await;
        assert_eq!(clear.text, "");
        let edit = put(&p, "desktop", "3", stale_clear.revision, "offline edit").await;
        assert_eq!(edit.text, "offline edit");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM notes_revisions")
                .fetch_one(&p)
                .await
                .unwrap(),
            5
        );
    }
    #[test]
    fn rich_conflicts_keep_images_and_escape_plain_text() {
        let merged = merge(
            &format!("{PREFIX}<img src=\"data:image/png;base64,AA==\">"),
            "<script> & text",
        );
        assert!(merged.contains("data:image/png"));
        assert!(merged.contains("&lt;script&gt; &amp; text"));
    }
}
