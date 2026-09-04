use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// Cap karakter fakta EXPLICIT per chat_id — melindungi budget hot tier di
/// system prompt (10k di context.rs). Explicit selalu masuk prompt, jadi ini
/// yang wajib ketat.
const MAX_EXPLICIT_CHARS: i64 = 9_000;

/// Safety net karakter total (explicit + inferred). Inferred hanya masuk
/// prompt via FTS recall (budget terpisah), jadi boleh bengkak — cap besar
/// ini hanya mencegah DB tumbuh tak terbatas.
const MAX_TOTAL_CHARS: i64 = 100_000;

/// Half-life hotness (hari) — adopsi OpenViking memory_lifecycle.
const HOTNESS_HALF_LIFE_DAYS: f64 = 7.0;

pub struct MemoryFact {
    pub id: i64,
    pub fact: String,
    pub fact_type: String,
    pub access_count: i64,
    pub accessed_at: DateTime<Utc>,
}

/// Hotness score (0.0–1.0): sigmoid(log1p(access_count)) * exp_decay(accessed_at).
/// Pure function — murah dipanggil, dipakai urutan recall & prioritas /dream.
pub fn hotness(access_count: i64, accessed_at: DateTime<Utc>, now: DateTime<Utc>) -> f64 {
    let n = access_count.max(0) as f64;
    let freq = 1.0 / (1.0 + (-((1.0 + n).ln())).exp()); // sigmoid(log1p(n))
    let age_days = (now - accessed_at).num_seconds().max(0) as f64 / 86_400.0;
    let decay = (-age_days * std::f64::consts::LN_2 / HOTNESS_HALF_LIFE_DAYS).exp();
    freq * decay
}

/// Tandai fakta barusan ter-inject ke prompt (dipanggil pasca recall).
pub async fn bump_access(pool: &PgPool, ids: &[i64]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    sqlx::query("UPDATE memory SET access_count = access_count + 1, accessed_at = now() WHERE id = ANY($1)")
        .bind(ids)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn save_fact(
    pool: &PgPool,
    chat_id: i64,
    fact: &str,
    fact_type: &str,
) -> Result<String> {
    let (used,): (i64,) =
        sqlx::query_as("SELECT COALESCE(SUM(LENGTH(fact)), 0) FROM memory WHERE chat_id = $1")
            .bind(chat_id)
            .fetch_one(pool)
            .await?;
    if fact_type == "explicit" {
        let (used_explicit,): (i64,) = sqlx::query_as(
            "SELECT COALESCE(SUM(LENGTH(fact)), 0) FROM memory WHERE chat_id = $1 AND type = 'explicit'",
        )
        .bind(chat_id)
        .fetch_one(pool)
        .await?;
        if used_explicit + fact.len() as i64 > MAX_EXPLICIT_CHARS {
            return Ok(format!(
                "⚠️ Cap explicit memory tercapai ({} karakter) — hot tier di \
                 system prompt akan membengkak. Fakta tidak disimpan: jalankan \
                 /dream untuk konsolidasi, atau /memory del <id>.",
                MAX_EXPLICIT_CHARS
            ));
        }
    }
    if used + fact.len() as i64 > MAX_TOTAL_CHARS {
        return Ok(format!(
            "⚠️ Memory safety net tercapai ({} karakter). Fakta tidak disimpan — \
             jalankan /dream untuk konsolidasi.",
            MAX_TOTAL_CHARS
        ));
    }
    sqlx::query("INSERT INTO memory (chat_id, fact, type) VALUES ($1, $2, $3)")
        .bind(chat_id)
        .bind(fact)
        .bind(fact_type)
        .execute(pool)
        .await?;
    Ok(format!("✅ Memory tersimpan [{}]: {}", fact_type, fact))
}

pub async fn list_facts(pool: &PgPool, chat_id: i64, limit: i64) -> Result<Vec<MemoryFact>> {
    let rows = sqlx::query_as::<_, (i64, String, String, i64, DateTime<Utc>)>(
        "SELECT id, fact, type, access_count, accessed_at FROM memory \
         WHERE chat_id = $1 ORDER BY id DESC LIMIT $2",
    )
    .bind(chat_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, fact, fact_type, access_count, accessed_at)| MemoryFact {
            id,
            fact,
            fact_type,
            access_count,
            accessed_at,
        })
        .collect())
}

pub async fn delete_fact(pool: &PgPool, chat_id: i64, id: i64) -> Result<bool> {
    let res = sqlx::query("DELETE FROM memory WHERE id = $1 AND chat_id = $2")
        .bind(id)
        .bind(chat_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// Chat id yang punya memory — dipakai dreaming cycle (iterasi per chat, Pilar 6).
pub async fn distinct_chat_ids(pool: &PgPool) -> Result<Vec<i64>> {
    let rows = sqlx::query_as::<_, (i64,)>("SELECT DISTINCT chat_id FROM memory")
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|(c,)| c).collect())
}

/// Tulis ulang teks fakta (dreaming: merge/rewrite) — id harus milik chat tsb.
/// Rewrite = freshness reset (accessed_at = now), mirip merge_op OpenViking.
pub async fn update_fact(pool: &PgPool, chat_id: i64, id: i64, fact: &str) -> Result<bool> {
    let res = sqlx::query(
        "UPDATE memory SET fact = $3, accessed_at = now() WHERE id = $1 AND chat_id = $2",
    )
    .bind(id)
    .bind(chat_id)
    .bind(fact)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Ubah tipe fakta (dreaming: upgrade inferred → explicit, Pilar 6).
pub async fn set_fact_type(pool: &PgPool, chat_id: i64, id: i64, fact_type: &str) -> Result<bool> {
    if !matches!(fact_type, "explicit" | "inferred") {
        anyhow::bail!("fact_type tidak valid: {fact_type}");
    }
    let res = sqlx::query("UPDATE memory SET type = $3 WHERE id = $1 AND chat_id = $2")
        .bind(id)
        .bind(chat_id)
        .bind(fact_type)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// Daftar fakta untuk disuntik ke system prompt (urut hotness — paling sering
/// & terakhir dipakai di atas, biar budget cut kena yang cold duluan).
pub async fn facts_for_prompt(pool: &PgPool, chat_id: i64) -> Result<String> {
    let facts = list_facts(pool, chat_id, 200).await?;
    if facts.is_empty() {
        return Ok("(belum ada fakta tersimpan)".into());
    }
    let now = Utc::now();
    let mut sorted = facts;
    sorted.sort_by(|a, b| {
        hotness(b.access_count, b.accessed_at, now)
            .partial_cmp(&hotness(a.access_count, a.accessed_at, now))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(sorted
        .iter()
        .map(|f| format!("- {} [{}]", f.fact, f.fact_type))
        .collect::<Vec<_>>()
        .join("\n"))
}

// ============ Memory v2 — recall-based injection (adopsi hermes-agent) ============

/// Budget karakter maksimal yang di-inject ke system prompt (recall).
const RECALL_BUDGET_CHARS: usize = 10_000;

/// Recall selectif: explicit selalu masuk (urut hotness); inferred hanya kalau
/// match FTS terhadap kata kunci pesan user, ATAU jika query kosong (scheduled
/// job). Output sudah didedup + dipotong budget. IDs yang ter-inject di-bump.
pub async fn recall_facts(pool: &PgPool, chat_id: i64, query: &str) -> Result<String> {
    // 1) Explicit — selalu (core identity owner, gak boleh hilang karena keyword miss).
    let explicit_rows = sqlx::query_as::<_, (i64, String, i64, DateTime<Utc>)>(
        "SELECT id, fact, access_count, accessed_at FROM memory \
         WHERE chat_id = $1 AND type = 'explicit' ORDER BY id",
    )
    .bind(chat_id)
    .fetch_all(pool)
    .await?;
    let now = Utc::now();
    let mut explicit_rows: Vec<(i64, String, i64, DateTime<Utc>)> = explicit_rows;
    explicit_rows.sort_by(|a, b| {
        hotness(b.2, b.3, now)
            .partial_cmp(&hotness(a.2, a.3, now))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let explicit: Vec<(i64, String)> =
        explicit_rows.into_iter().map(|(id, fact, ..)| (id, fact)).collect();

    // 2) Inferred — FTS match kalau ada query, urut rank.
    //    AND semua kata pesan user terlalu ketat (pesan panjang = pasti 0 hasil),
    //    jadi: ekstrak keywords (>=4 char, alnum), gabung OR, urut rank, threshold.
    let mut inferred: Vec<(i64, String)> = Vec::new();
    let kws: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 4)
        .map(String::from)
        .take(12)
        .collect();
    if !kws.is_empty() {
        let or_query = kws.join(" | ");
        inferred = sqlx::query_as::<_, (i64, String)>(
            r#"SELECT id, fact FROM memory
               WHERE chat_id = $1 AND type = 'inferred'
                 AND search_vector @@ to_tsquery('simple', $2)
                 AND ts_rank(search_vector, to_tsquery('simple', $2)) > 0.01
               ORDER BY ts_rank(search_vector, to_tsquery('simple', $2)) DESC
               LIMIT 30"#,
        )
        .bind(chat_id)
        .bind(&or_query)
        .fetch_all(pool)
        .await
        .unwrap_or_default(); // FTS gagal (misal syntax aneh) -> inferred kosong, bukan error
    }

    // 3) Gabung + dedup (substring-case-insensitive; pola duplikat semantik "backup v1/v2").
    let mut seen: Vec<String> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    let mut injected_ids: Vec<i64> = Vec::new();
    let mut used = 0usize;
    for (id, fact, ..) in explicit.iter().chain(inferred.iter()) {
        let low = fact.to_lowercase();
        if seen.iter().any(|s| low.contains(s.as_str()) || s.contains(&low)) {
            continue;
        }
        let line = format!("- {} [id:{}]", fact, id);
        if used + line.len() > RECALL_BUDGET_CHARS {
            break;
        }
        used += line.len();
        seen.push(low);
        injected_ids.push(*id);
        lines.push(line);
    }
    bump_access(pool, &injected_ids).await?;
    if lines.is_empty() {
        return Ok("(belum ada fakta tersimpan)".into());
    }
    Ok(lines.join("\n"))
}

/// Filter entri transien/trivial — jangan sampai intermediate step masuk long-term memory.
pub fn is_transient_fact(fact: &str) -> bool {
    let f = fact.trim();
    let low = f.to_lowercase();
    let transient_markers = [
        "owner setuju", "disetujui", "approved", "kandidat delete", "kandidat prune",
        "menunggu konfirmasi", "menunggu keputusan", "menunggu owner", "akan dieksekusi",
        "sedang dikerjakan", "sebentar", "in progress", "todo:", "queue ke antrian",
        "masuk antrian", "langkah berikutnya",
    ];
    if transient_markers.iter().any(|m| low.contains(m)) {
        return true;
    }
    // Entri pendek tanpa angka/URL/path → terlalu tipis jadi fakta stabil.
    let has_anchor = f.chars().any(|c| c.is_ascii_digit())
        || low.contains("http")
        || low.contains('/')
        || low.contains("rp ");
    f.chars().count() < 25 && !has_anchor
}

/// Semantik-dedup murah: cari entri existing dengan ts_rank sangat tinggi.
/// Kalau ketemu → return id-nya (caller UPDATE, bukan INSERT).
pub async fn find_near_duplicate(
    pool: &PgPool,
    chat_id: i64,
    fact: &str,
) -> Result<Option<i64>> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"SELECT id FROM memory
           WHERE chat_id = $1
             AND search_vector @@ websearch_to_tsquery('simple', $2)
             AND ts_rank(search_vector, websearch_to_tsquery('simple', $2)) > 0.6
           ORDER BY ts_rank(search_vector, websearch_to_tsquery('simple', $2)) DESC
           LIMIT 1"#,
    )
    .bind(chat_id)
    .bind(fact)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id,)| id))
}

#[cfg(test)]
mod tests_v2 {
    use super::*;
    use chrono::Duration;

    #[test]
    fn transient_markers_detected() {
        assert!(is_transient_fact("owner setuju opsi 1"));
        assert!(is_transient_fact("menunggu keputusan owner soal race"));
        assert!(is_transient_fact("masuk antrian patch berikutnya"));
    }

    #[test]
    fn short_trivial_rejected() {
        assert!(is_transient_fact("ok sip"));
        assert!(is_transient_fact("lanjut besok"));
    }

    #[test]
    fn real_facts_pass() {
        assert!(!is_transient_fact("owner lari sore biasanya jam 17:00 di Stadion REDACTED-CITY"));
        assert!(!is_transient_fact("email SMTP pakai example.com port 465 SSL"));
        assert!(!is_transient_fact("budget makan harian Rp 35.000"));
    }

    #[test]
    fn keyword_extraction_ok() {
        let q = "kalo program running ku gimana? pace Z2 ok ga".to_lowercase();
        let kws: Vec<&str> = q
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.chars().count() >= 4)
            .take(12)
            .collect();
        assert!(kws.contains(&"running") && kws.contains(&"program"));
        let joined = kws.join(" | ");
        assert!(joined.contains('|'));
    }

    #[test]
    fn hotness_freq_and_decay() {
        let now = Utc::now();
        // Akses 0x vs 10x, sama-sama fresh: yang sering diakses lebih panas.
        let fresh = now - Duration::hours(1);
        let cold_acc = hotness(0, fresh, now);
        let hot_acc = hotness(10, fresh, now);
        assert!(hot_acc > cold_acc);
        // Akses sama, tapi sudah lama tidak diakses: meluruh.
        let stale = hotness(10, now - Duration::days(30), now);
        assert!(stale < hot_acc * 0.5);
        // Semua nilai di rentang valid.
        assert!((0.0..=1.0).contains(&hot_acc));
        assert!((0.0..=1.0).contains(&stale));
    }
}
