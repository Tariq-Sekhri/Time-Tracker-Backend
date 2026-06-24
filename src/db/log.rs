use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct Log{
    pub id:i64,
    pub device_uuid:String,
    pub app:String,
    pub timestamp:i64,
    pub duration:i64,
}

pub async fn create_log_table(pool: &SqlitePool) -> anyhow::Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS logs (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        device_uuid TEXT NOT NULL,
        app TEXT NOT NULL,
        timestamp INTEGER NOT NULL,
        duration INTEGER NOT NULL DEFAULT 0
    )",
    )
        .execute(pool)
        .await?;
    Ok(())
}