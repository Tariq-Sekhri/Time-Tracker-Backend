use serde::{Deserialize, Serialize};
use sqlx::{Error, FromRow, SqlitePool};
use sqlx::sqlite::SqliteQueryResult;
use anyhow::Result;
use crate::db::device::Device;

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct Log{
    pub id:i64,
    pub device_uuid:String,
    pub app:String,
    pub timestamp:i64,
    pub duration:i64,
}

pub async fn create_log_table(pool: &SqlitePool) -> Result<()> {
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

pub async fn insert_log(pool: &SqlitePool, log: Log) -> Result<()> {
    sqlx::query("insert into logs (device_uuid, app, timestamp, duration) values  (?, ?,?,?)").bind(log.device_uuid).bind(log.app).bind(log.timestamp).bind(log.duration).execute(pool).await?;
    Ok(())
}

pub async fn delete_log(pool: &SqlitePool, id:i64, device_uuid: String) -> Result<()> {
    sqlx::query("delete from log where device_uuid=? AND device_uuid=?").bind(device_uuid).bind(id).execute(pool).await?;
    Ok(())
}