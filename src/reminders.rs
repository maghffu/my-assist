use anyhow::{bail, Result};
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use sqlx::PgPool;

pub struct Reminder {
    pub id: i64,
    pub chat_id: i64,
    pub message: String,
    pub remind_at: DateTime<Utc>,
    pub kind: String,
    pub recur: Option<String>,
}

pub async fn create(
    pool: &PgPool,
    chat_id: i64,
    message: &str,
    remind_at: DateTime<Utc>,
    kind: &str,
    recur: Option<String>,
) -> Result<i64> {
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO reminders (chat_id, message, remind_at, kind, recur)
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(chat_id)
    .bind(message)
    .bind(remind_at)
    .bind(kind)
    .bind(recur)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

pub async fn due_now(pool: &PgPool) -> Result<Vec<Reminder>> {
    let rows = sqlx::query_as::<_, (i64, i64, String, DateTime<Utc>, String, Option<String>)>(
        "SELECT id, chat_id, message, remind_at, kind, recur
         FROM reminders
         WHERE sent = false AND remind_at <= now()
         ORDER BY remind_at ASC
         LIMIT 20",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, chat_id, message, remind_at, kind, recur)| Reminder {
            id,
            chat_id,
            message,
            remind_at,
            kind,
            recur,
        })
        .collect())
}

pub async fn mark_sent(pool: &PgPool, id: i64) -> Result<()> {
    sqlx::query("UPDATE reminders SET sent = true WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Reminder berulang (Pilar 4): reschedule, bukan mark sent.
pub async fn reschedule(pool: &PgPool, id: i64, next: DateTime<Utc>) -> Result<()> {
    sqlx::query("UPDATE reminders SET remind_at = $2 WHERE id = $1")
        .bind(id)
        .bind(next)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn list_pending(pool: &PgPool, chat_id: i64, limit: i64) -> Result<Vec<Reminder>> {
    let rows = sqlx::query_as::<_, (i64, i64, String, DateTime<Utc>, String, Option<String>)>(
        "SELECT id, chat_id, message, remind_at, kind, recur
         FROM reminders
         WHERE sent = false AND chat_id = $1
         ORDER BY remind_at ASC
         LIMIT $2",
    )
    .bind(chat_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, chat_id, message, remind_at, kind, recur)| Reminder {
            id,
            chat_id,
            message,
            remind_at,
            kind,
            recur,
        })
        .collect())
}

pub async fn delete(pool: &PgPool, chat_id: i64, id: i64) -> Result<bool> {
    let res = sqlx::query("DELETE FROM reminders WHERE id = $1 AND chat_id = $2")
        .bind(id)
        .bind(chat_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// 'daily' | 'weekly' (cron expression menyusul — ROADMAP Fase 3 note).
pub fn compute_next_run(recur: &str, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
    match recur {
        "daily" => Some(from + Duration::days(1)),
        "weekly" => Some(from + Duration::weeks(1)),
        _ => None,
    }
}

/// Parse waktu dari tool call LLM: RFC 3339 dengan timezone,
/// fallback "YYYY-MM-DD HH:MM" dianggap waktu lokal owner (UTC+7).
pub fn parse_remind_at(s: &str) -> Result<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(s.trim(), "%Y-%m-%d %H:%M") {
        let tz = chrono::FixedOffset::east_opt(7 * 3600).unwrap();
        return match naive.and_local_timezone(tz).single() {
            Some(dt) => Ok(dt.with_timezone(&Utc)),
            None => bail!("waktu tidak valid: {}", s),
        };
    }
    bail!(
        "format waktu tidak dikenali: {:?} — pakai RFC 3339, contoh 2025-06-01T15:00:00+07:00",
        s
    )
}

/// Tampilkan waktu dalam zona owner (UTC+7).
pub fn fmt_jakarta(dt: DateTime<Utc>) -> String {
    let tz = chrono::FixedOffset::east_opt(7 * 3600).unwrap();
    dt.with_timezone(&tz).format("%Y-%m-%d %H:%M UTC+7").to_string()
}
