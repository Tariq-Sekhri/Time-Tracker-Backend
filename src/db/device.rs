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
    pub uuid: Uuid,
    pub hash_token:String,
    pub name: String,
    pub last_sync_id:i64,
}
#[derive(Debug, Serialize, Deserialize, FromRow, Clone)]
pub struct PubDevice {
    pub uuid: Uuid,
    pub name: String,
    pub last_sync_id:i64,
}

impl Device {
    pub fn new(name:String, token:&String)->Result<Self>{
        let uuid = Uuid::new_v4();
        let hash_token= hash_token(&token);

        Ok(Self{uuid,hash_token,name,last_sync_id:0})
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
            last_sync_id INTEGER NOT NULL DEFAULT 0
        )",
    )
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn insert_device(pool:&SqlitePool, device:Device)->Result<()>{
    sqlx::query("insert into devices (uuid, hash_token, name)  values (?, ? ,?)").bind(device.uuid).bind(device.hash_token).bind(device.name).execute(pool).await?;
    Ok(())
}

pub async fn get_device_by_raw_token(pool: &SqlitePool, token: String) -> Result<Device, sqlx::Error> {
    let hash = hash_token(&token);
    sqlx::query_as("select * from devices where hash_token=?")
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
    sqlx::query("UPDATE devices SET last_sync_id = ? WHERE uuid = ?")
        .bind(new_id)
        .bind(device_uuid)
        .execute(executor)
        .await?;
    Ok(())
}