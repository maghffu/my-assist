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
    pub fail_count: i32,
}

/// Maksimum backoff antar retry (8 jam) — utk recurring/one-shot yg gagal terus,
/// interval naik eksponensial 15m → 30m → 1h → ... sampai cap ini.
pub const MAX_BACKOFF: i64 = 8 * 3600;
/// Menyerah setelah kumulatif backoff melewati ambang ini (24 jam) utk one-shot:
/// kalau 24 jam berturut gagal, tandai sent (biar tidak selamanya dipukul).
pub const GIVEUP_AFTER_SECS: i64 = 24 * 3600;

/// Backoff eksponensial (detik) berdasarkan jumlah kegagalan beruntun.
/// 2^(n-1) * 15 menit, dinaikkan bertahap, di-cap MAX_BACKOFF.
pub fn backoff_secs(fail_count: i32) -> i64 {
    let n = fail_count.max(1) as u32;
    let base: i64 = 15 * 60;
    // Hindari overflow utk n besar; shift bisa overflow di atas 63 bit.
    let pow = if n > 20 {
        MAX_BACKOFF
    } else {
        base.saturating_mul(1i64 << (n - 1))
    };
    pow.min(MAX_BACKOFF)
}

/// Total backoff kumulatif (detik) dari fail_count kegagalan beruntun —
/// dipakai utk menentukan kapan one-shot reminder "menyerah" (GIVEUP_AFTER_SECS).
pub fn cumulative_backoff_secs(fail_count: i32) -> i64 {
    let mut total = 0i64;
    // Batasi loop utk hindari kerja sia-sia saat fail_count sudah besar.
    for n in 1..=fail_count {
        total = total.saturating_add(backoff_secs(n));
        if total >= GIVEUP_AFTER_SECS {
            break;
        }
    }
    total
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
    let rows = sqlx::query_as::<_, (i64, i64, String, DateTime<Utc>, String, Option<String>, i32)>(
        "SELECT id, chat_id, message, remind_at, kind, recur, fail_count
         FROM reminders
         WHERE sent = false AND remind_at <= now()
         ORDER BY remind_at ASC
         LIMIT 20",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, chat_id, message, remind_at, kind, recur, fail_count)| Reminder {
            id,
            chat_id,
            message,
            remind_at,
            kind,
            recur,
            fail_count,
        })
        .collect())
}

/// Increment fail_count + push remind_at mundur dengan backoff eksponensial.
/// THREAD gagal job/reminder supaya TIDAK retry tiap 30 detik membabi buta.
/// Kembalikan fail_count yang baru (sudah +1).
pub async fn bump_fail(pool: &PgPool, id: i64, now: DateTime<Utc>, prev_fail: i32) -> Result<i32> {
    let new_fail = prev_fail.saturating_add(1);
    let delay = chrono::Duration::seconds(backoff_secs(new_fail));
    sqlx::query(
        "UPDATE reminders SET fail_count = $2, remind_at = $3 WHERE id = $1",
    )
    .bind(id)
    .bind(new_fail)
    .bind(now + delay)
    .execute(pool)
    .await?;
    Ok(new_fail)
}

/// Reset fail_count setelah pengiriman sukses.
pub async fn reset_fail(pool: &PgPool, id: i64) -> Result<()> {
    sqlx::query("UPDATE reminders SET fail_count = 0 WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
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
    let rows = sqlx::query_as::<_, (i64, i64, String, DateTime<Utc>, String, Option<String>, i32)>(
        "SELECT id, chat_id, message, remind_at, kind, recur, fail_count
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
        .map(|(id, chat_id, message, remind_at, kind, recur, fail_count)| Reminder {
            id,
            chat_id,
            message,
            remind_at,
            kind,
            recur,
            fail_count,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_naik_eksponensial_lalu_cap() {
        // 15m, 30m, 1h, 2h, 4h, 8h, lalu cap 8h.
        assert_eq!(backoff_secs(1), 15 * 60);
        assert_eq!(backoff_secs(2), 30 * 60);
        assert_eq!(backoff_secs(3), 60 * 60);
        assert_eq!(backoff_secs(4), 2 * 3600);
        assert_eq!(backoff_secs(5), 4 * 3600);
        assert_eq!(backoff_secs(6), 8 * 3600);
        assert_eq!(backoff_secs(100), MAX_BACKOFF);
    }

    #[test]
    fn cumulative_menyerah_setelah_24_jam() {
        // fail_count 1..=7 → 15+30+60+120+240+480+480 = 1425 menit = 23.75 jam ≤ 24
        assert!(cumulative_backoff_secs(7) <= GIVEUP_AFTER_SECS);
        // fail_count 8 → +480 menit = 31.75 jam > 24 → menyerah
        assert!(cumulative_backoff_secs(8) > GIVEUP_AFTER_SECS);
    }
}
