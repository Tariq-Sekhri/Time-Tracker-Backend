pub mod log;
pub mod device;


use sqlx::{SqlitePool};
use crate::db::device::create_devices_table;
use crate::db::log::{create_log_table, ensure_logs_schema};

pub static  DATABASE_URL:&str ="sqlite://server.db";
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
}


pub async fn check_state(pool: &SqlitePool)-> anyhow::Result<()> {
    create_devices_table(pool).await?;
    create_log_table(pool).await?;
    ensure_logs_schema(pool).await?;
    Ok(())
}
