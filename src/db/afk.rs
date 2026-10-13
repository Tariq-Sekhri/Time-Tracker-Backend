use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};

pub const PAGE_SIZE: usize = 500;

#[derive(Clone, Debug, Deserialize, Serialize, FromRow)]
pub struct Evidence {
    pub id: i64,
    pub start_time: i64,
    pub end_time: i64,
    pub state: i32,
    pub known_mask: i64,
    pub active_mask: i64,
    pub settings_json: String,
    #[serde(default)]
    pub revision: i64,
}

impl Evidence {
    pub fn valid(&self) -> bool {
        self.id > 0
            && self.start_time >= 0
            && self.end_time > self.start_time
            && (0..=2).contains(&self.state)
            && (0..=2047).contains(&self.known_mask)
            && (0..=2047).contains(&self.active_mask)
            && self.active_mask & !self.known_mask == 0
            && self.settings_json.len() <= 16_384
            && serde_json::from_str::<serde_json::Value>(&self.settings_json)
                .is_ok_and(|v| v.is_object())
    }
}

pub async fn ensure_schema(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS afk_configurations (
        id INTEGER PRIMARY KEY, settings_json TEXT NOT NULL UNIQUE
    );
    CREATE TABLE IF NOT EXISTS afk_revision (
        singleton INTEGER PRIMARY KEY CHECK(singleton=1), value INTEGER NOT NULL
    );
    INSERT OR IGNORE INTO afk_revision VALUES (1,0);
    CREATE TABLE IF NOT EXISTS afk_intervals (
        device_uuid TEXT NOT NULL REFERENCES devices(uuid) ON DELETE CASCADE,
        id INTEGER NOT NULL, start_time INTEGER NOT NULL, end_time INTEGER NOT NULL,
        state INTEGER NOT NULL, known_mask INTEGER NOT NULL, active_mask INTEGER NOT NULL,
        settings_id INTEGER NOT NULL REFERENCES afk_configurations(id), revision INTEGER NOT NULL,
        PRIMARY KEY(device_uuid,id)
    );
    CREATE INDEX IF NOT EXISTS idx_afk_revision ON afk_intervals(device_uuid,revision);",
    )
    .execute(pool)
    .await?;
    Ok(())
}

// Device identity comes from authentication, never from the submitted evidence.
// Replays are idempotent; extending the recorder's tail creates a new revision.
pub async fn upload(
    pool: &SqlitePool,
    device: &str,
    evidence: &[Evidence],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    for e in evidence {
        sqlx::query("INSERT OR IGNORE INTO afk_configurations(settings_json) VALUES (?)")
            .bind(&e.settings_json)
            .execute(&mut *tx)
            .await?;
        let settings_id: i64 =
            sqlx::query_scalar("SELECT id FROM afk_configurations WHERE settings_json=?")
                .bind(&e.settings_json)
                .fetch_one(&mut *tx)
                .await?;
        let same: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM afk_intervals WHERE device_uuid=? AND id=? AND start_time=? AND end_time>=? AND state=? AND known_mask=? AND active_mask=? AND settings_id=?)")
            .bind(device).bind(e.id).bind(e.start_time).bind(e.end_time).bind(e.state)
            .bind(e.known_mask).bind(e.active_mask).bind(settings_id).fetch_one(&mut *tx).await?;
        if same {
            continue;
        }
        let revision: i64 = sqlx::query_scalar(
            "UPDATE afk_revision SET value=value+1 WHERE singleton=1 RETURNING value",
        )
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO afk_intervals(device_uuid,id,start_time,end_time,state,known_mask,active_mask,settings_id,revision)
            VALUES (?,?,?,?,?,?,?,?,?) ON CONFLICT(device_uuid,id) DO UPDATE SET
            start_time=excluded.start_time,end_time=excluded.end_time,state=excluded.state,
            known_mask=excluded.known_mask,active_mask=excluded.active_mask,settings_id=excluded.settings_id,revision=excluded.revision")
            .bind(device).bind(e.id).bind(e.start_time).bind(e.end_time).bind(e.state)
            .bind(e.known_mask).bind(e.active_mask).bind(settings_id).bind(revision)
            .execute(&mut *tx).await?;
    }
    tx.commit().await
}

pub async fn download(
    pool: &SqlitePool,
    device: &str,
    after: i64,
) -> Result<Vec<Evidence>, sqlx::Error> {
    sqlx::query_as("SELECT a.id,a.start_time,a.end_time,a.state,a.known_mask,a.active_mask,c.settings_json,a.revision
        FROM afk_intervals a JOIN afk_configurations c ON c.id=a.settings_id
        WHERE a.device_uuid=? AND a.revision>? ORDER BY a.revision LIMIT ?")
        .bind(device).bind(after).bind(PAGE_SIZE as i64).fetch_all(pool).await
}
