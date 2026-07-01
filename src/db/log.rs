use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct Log {
    pub id: i64,
    pub device_uuid: String,
    pub app: String,
    pub timestamp: i64,
    pub duration: i64,
}

pub async fn create_log_table(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "
        CREATE TABLE IF NOT EXISTS logs (
            id INTEGER NOT NULL,
            device_uuid TEXT NOT NULL,
            app TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            duration INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (device_uuid, id),
            FOREIGN KEY (device_uuid)
                REFERENCES devices(uuid)
                ON DELETE CASCADE
        )
        ",
    )
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn ensure_logs_schema(pool: &SqlitePool) -> Result<()> {
    migrate_logs_composite_primary_key(pool).await
}

async fn migrate_logs_composite_primary_key(pool: &SqlitePool) -> Result<()> {
    let ddl: Option<String> =
        sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'logs'")
            .fetch_optional(pool)
            .await?;

    let Some(ddl) = ddl else {
        return Ok(());
    };

    if ddl.contains("PRIMARY KEY (device_uuid, id)") {
        return Ok(());
    }

    sqlx::query(
        "
        DELETE FROM logs
        WHERE device_uuid IS NULL OR device_uuid = ''
        ",
    )
    .execute(pool)
    .await?;

    let mut tx = pool.begin().await?;

    sqlx::query(
        "
        CREATE TABLE logs_new (
            id INTEGER NOT NULL,
            device_uuid TEXT NOT NULL,
            app TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            duration INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (device_uuid, id),
            FOREIGN KEY (device_uuid)
                REFERENCES devices(uuid)
                ON DELETE CASCADE
        )
        ",
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "
        INSERT OR IGNORE INTO logs_new (id, device_uuid, app, timestamp, duration)
        SELECT id, device_uuid, app, timestamp, duration FROM logs
        WHERE device_uuid IS NOT NULL AND device_uuid != ''
        ",
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query("DROP TABLE logs").execute(&mut *tx).await?;

    sqlx::query("ALTER TABLE logs_new RENAME TO logs")
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(())
}
