use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::rngs::SysRng;
use rand::TryRng;
use sha2::{Digest, Sha256};
use anyhow::Result;

#[derive(Debug, Serialize, Deserialize, FromRow, Clone)]
pub struct Device {
    pub uuid: String,
    pub hash_token:String,
    pub name: String,
    pub last_sync_id:i64,
    pub is_active: bool,
}
#[derive(Debug, Serialize, Deserialize, FromRow, Clone)]
pub struct PubDevice {
    pub uuid: String,
    pub name: String,
    pub last_sync_id:i64,
    pub is_active: bool,
}

impl Device {
    pub fn new(name:String, token:&String)->Result<Self>{
        let uuid = Uuid::new_v4().to_string();
        let hash_token= hash_token(&token);

        Ok(Self{uuid,hash_token,name,last_sync_id:0,is_active:false})
    }
}


pub fn generate_auth_token() -> Result<String> {
    let mut bytes = [0u8; 32];

    SysRng.try_fill_bytes(&mut bytes)?;

    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn hash_token(token: &str) -> String {
    let hash = Sha256::digest(token.as_bytes());

    URL_SAFE_NO_PAD.encode(hash)
}

pub async fn create_devices_table(
    pool: &SqlitePool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS devices (
            uuid TEXT PRIMARY KEY NOT NULL,
            hash_token TEXT NOT NULL,
            name TEXT NOT NULL,
            last_sync_id INTEGER NOT NULL DEFAULT 0,
            is_active INTEGER NOT NULL DEFAULT 0
        )",
    )
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn ensure_devices_schema(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let columns: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('devices')")
        .fetch_all(pool)
        .await?;
    if !columns.iter().any(|column| column == "is_active") {
        sqlx::query("ALTER TABLE devices ADD COLUMN is_active INTEGER NOT NULL DEFAULT 0")
            .execute(pool)
            .await?;
    }
    Ok(())
}

pub async fn insert_device(pool:&SqlitePool, device:Device)->Result<()>{
    sqlx::query("insert into devices (uuid, hash_token, name, is_active) values (?, ?, ?, ?)")
        .bind(device.uuid)
        .bind(device.hash_token)
        .bind(device.name)
        .bind(device.is_active)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get_device_by_raw_token(pool: &SqlitePool, token: String) -> Result<Device, sqlx::Error> {
    let hash = hash_token(&token);
    sqlx::query_as("SELECT * FROM devices WHERE hash_token = ?")
        .bind(hash)
        .fetch_one(pool)
        .await
}

pub async fn update_last_sync_id<'e, E>(
    executor: E,
    device_uuid: &str,
    new_id: i64,
) -> Result<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query("UPDATE devices SET last_sync_id = MAX(last_sync_id, ?) WHERE uuid = ?")
        .bind(new_id)
        .bind(device_uuid)
        .execute(executor)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn last_sync_id_never_regresses_on_retried_out_of_order_requests() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        create_devices_table(&pool).await.unwrap();

        let token = "test-token".to_string();
        let device = Device::new("test-device".to_string(), &token).unwrap();
        let uuid = device.uuid.clone();
        insert_device(&pool, device).await.unwrap();
        sqlx::query("UPDATE devices SET is_active = 1 WHERE uuid = ?")
            .bind(&uuid)
            .execute(&pool)
            .await
            .unwrap();

        update_last_sync_id(&pool, &uuid, 10).await.unwrap();
        update_last_sync_id(&pool, &uuid, 4).await.unwrap();

        let last_sync_id: i64 =
            sqlx::query_scalar("SELECT last_sync_id FROM devices WHERE uuid = ?")
                .bind(uuid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(last_sync_id, 10);
    }

    #[tokio::test]
    async fn legacy_devices_become_inactive_when_approval_is_added() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE devices (
                uuid TEXT PRIMARY KEY NOT NULL,
                hash_token TEXT NOT NULL,
                name TEXT NOT NULL,
                last_sync_id INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO devices (uuid, hash_token, name) VALUES ('legacy', 'hash', 'Legacy')",
        )
        .execute(&pool)
        .await
        .unwrap();

        ensure_devices_schema(&pool).await.unwrap();

        let is_active: bool =
            sqlx::query_scalar("SELECT is_active FROM devices WHERE uuid = 'legacy'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!is_active);
    }
}
