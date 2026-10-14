use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};

pub const PAGE_SIZE: i64 = 500;

/// The one manual timer shared by every approved device. `session_id` is chosen
/// by the device that started it so retries and offline replays stay idempotent.
#[derive(Clone, Debug, Deserialize, Serialize, FromRow, PartialEq)]
pub struct Timer {
    pub session_id: String,
    pub title: String,
    pub project_name: Option<String>,
    pub start_time: i64,
    #[serde(default)]
    pub version: i64,
    #[serde(default)]
    pub device_uuid: String,
}

/// A finished manual time block. `id` is the download cursor.
#[derive(Clone, Debug, Deserialize, Serialize, FromRow, PartialEq)]
pub struct Entry {
    #[serde(default)]
    pub id: i64,
    pub session_id: String,
    pub title: String,
    pub project_name: Option<String>,
    pub start_time: i64,
    pub end_time: i64,
    #[serde(default)]
    pub device_uuid: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, FromRow, PartialEq)]
pub struct Project {
    pub name: String,
    pub color: String,
}

pub enum PutTimer {
    Saved(Timer),
    /// Another session is already running; the caller should adopt it.
    Conflict(Timer),
}

pub fn valid_text(title: &str, project: Option<&str>) -> bool {
    title.chars().count() <= 200
        && !title.chars().any(char::is_control)
        && project.is_none_or(|p| !p.trim().is_empty() && p.chars().count() <= 100)
}

pub fn valid_session(session_id: &str) -> bool {
    (1..=64).contains(&session_id.len())
        && session_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

pub fn valid_color(color: &str) -> bool {
    color.len() == 7 && color.starts_with('#') && color[1..].bytes().all(|c| c.is_ascii_hexdigit())
}

pub async fn ensure_schema(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS manual_timer (
        singleton INTEGER PRIMARY KEY CHECK(singleton=1),
        session_id TEXT NOT NULL, title TEXT NOT NULL, project_name TEXT,
        start_time INTEGER NOT NULL, version INTEGER NOT NULL, device_uuid TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS manual_entries (
        id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL UNIQUE,
        title TEXT NOT NULL, project_name TEXT, start_time INTEGER NOT NULL,
        end_time INTEGER NOT NULL, device_uuid TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS manual_projects (
        name TEXT PRIMARY KEY COLLATE NOCASE, color TEXT NOT NULL
    );",
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_timer(pool: &SqlitePool) -> Result<Option<Timer>, sqlx::Error> {
    sqlx::query_as(
        "SELECT session_id,title,project_name,start_time,version,device_uuid FROM manual_timer",
    )
    .fetch_optional(pool)
    .await
}

/// Starts a timer, or updates it when `timer.session_id` is the one running.
/// A session that already finished is refused so a late retry cannot revive it.
pub async fn put_timer(
    pool: &SqlitePool,
    device: &str,
    timer: &Timer,
) -> Result<Option<PutTimer>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let finished: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM manual_entries WHERE session_id=?)")
            .bind(&timer.session_id)
            .fetch_one(&mut *tx)
            .await?;
    if finished {
        return Ok(None);
    }
    let current: Option<Timer> = sqlx::query_as(
        "SELECT session_id,title,project_name,start_time,version,device_uuid FROM manual_timer",
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(current) = current.as_ref().filter(|c| c.session_id != timer.session_id) {
        return Ok(Some(PutTimer::Conflict(current.clone())));
    }
    let saved = Timer {
        version: current.map_or(1, |c| c.version + 1),
        device_uuid: device.to_string(),
        ..timer.clone()
    };
    sqlx::query(
        "INSERT INTO manual_timer(singleton,session_id,title,project_name,start_time,version,device_uuid)
         VALUES (1,?,?,?,?,?,?) ON CONFLICT(singleton) DO UPDATE SET
         title=excluded.title,project_name=excluded.project_name,start_time=excluded.start_time,
         version=excluded.version,device_uuid=excluded.device_uuid",
    )
    .bind(&saved.session_id)
    .bind(&saved.title)
    .bind(&saved.project_name)
    .bind(saved.start_time)
    .bind(saved.version)
    .bind(&saved.device_uuid)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(PutTimer::Saved(saved)))
}

/// Records a finished session and clears the running timer if it is that
/// session. Replays return the entry that was stored the first time.
/// Returns whether the running timer was cleared.
pub async fn finish(
    pool: &SqlitePool,
    device: &str,
    entry: &Entry,
) -> Result<(Entry, bool), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT OR IGNORE INTO manual_entries(session_id,title,project_name,start_time,end_time,device_uuid)
         VALUES (?,?,?,?,?,?)",
    )
    .bind(&entry.session_id)
    .bind(&entry.title)
    .bind(&entry.project_name)
    .bind(entry.start_time)
    .bind(entry.end_time)
    .bind(device)
    .execute(&mut *tx)
    .await?;
    let stored: Entry = sqlx::query_as(
        "SELECT id,session_id,title,project_name,start_time,end_time,device_uuid
         FROM manual_entries WHERE session_id=?",
    )
    .bind(&entry.session_id)
    .fetch_one(&mut *tx)
    .await?;
    let cleared = sqlx::query("DELETE FROM manual_timer WHERE session_id=?")
        .bind(&entry.session_id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        > 0;
    tx.commit().await?;
    Ok((stored, cleared))
}

pub async fn entries_after(pool: &SqlitePool, after: i64) -> Result<Vec<Entry>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id,session_id,title,project_name,start_time,end_time,device_uuid
         FROM manual_entries WHERE id>? ORDER BY id LIMIT ?",
    )
    .bind(after)
    .bind(PAGE_SIZE)
    .fetch_all(pool)
    .await
}

pub async fn get_projects(pool: &SqlitePool) -> Result<Vec<Project>, sqlx::Error> {
    sqlx::query_as("SELECT name,color FROM manual_projects ORDER BY name COLLATE NOCASE")
        .fetch_all(pool)
        .await
}

/// Merges a device's project list; projects are matched by name across devices.
pub async fn put_projects(pool: &SqlitePool, projects: &[Project]) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let mut changed = false;
    for project in projects {
        changed |= sqlx::query(
            "INSERT INTO manual_projects(name,color) VALUES (?,?)
             ON CONFLICT(name) DO UPDATE SET name=excluded.name,color=excluded.color
             WHERE manual_projects.name IS NOT excluded.name OR manual_projects.color IS NOT excluded.color",
        )
        .bind(project.name.trim())
        .bind(project.color.to_ascii_lowercase())
        .execute(&mut *tx)
        .await?
        .rows_affected()
            > 0;
    }
    tx.commit().await?;
    Ok(changed)
}
