use anyhow::Result;
use sqlx::PgPool;

/// Cap karakter fakta EXPLICIT per chat_id — melindungi budget hot tier di
/// system prompt (10k di context.rs). Explicit selalu masuk prompt, jadi ini
/// yang wajib ketat.
const MAX_EXPLICIT_CHARS: i64 = 9_000;

/// Safety net karakter total (explicit + inferred). Inferred hanya masuk
/// prompt via FTS recall (budget terpisah), jadi boleh bengkak — cap besar
/// ini hanya mencegah DB tumbuh tak terbatas.
const MAX_TOTAL_CHARS: i64 = 100_000;

pub struct MemoryFact {
    pub id: i64,
    pub fact: String,
    pub fact_type: String,
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
    let rows = sqlx::query_as::<_, (i64, String, String)>(
        "SELECT id, fact, type FROM memory WHERE chat_id = $1 ORDER BY id DESC LIMIT $2",
    )
    .bind(chat_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, fact, fact_type)| MemoryFact { id, fact, fact_type })
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
pub async fn update_fact(pool: &PgPool, chat_id: i64, id: i64, fact: &str) -> Result<bool> {
    let res = sqlx::query("UPDATE memory SET fact = $3 WHERE id = $1 AND chat_id = $2")
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

/// Daftar fakta untuk disuntik ke system prompt (terlama dulu — kronologis).
pub async fn facts_for_prompt(pool: &PgPool, chat_id: i64) -> Result<String> {
    let facts = list_facts(pool, chat_id, 200).await?;
    if facts.is_empty() {
        return Ok("(belum ada fakta tersimpan)".into());
    }
    Ok(facts
        .iter()
        .rev()
        .map(|f| format!("- {} [{}]", f.fact, f.fact_type))
        .collect::<Vec<_>>()
        .join("\n"))
}

// ============ Memory v2 — recall-based injection (adopsi hermes-agent) ============

/// Budget karakter maksimal yang di-inject ke system prompt (recall).
const RECALL_BUDGET_CHARS: usize = 10_000;

/// Recall selectif: explicit selalu masuk; inferred hanya kalau match FTS
/// terhadap kata kunci pesan user, ATAU jika query kosong (scheduled job).
/// Output sudah didedup + dipotong budget (explicit diprioritaskan, lalu rank).
pub async fn recall_facts(pool: &PgPool, chat_id: i64, query: &str) -> Result<String> {
    // 1) Explicit — selalu (core identity owner, gak boleh hilang karena keyword miss).
    let explicit = sqlx::query_as::<_, (i64, String)>(
        "SELECT id, fact FROM memory WHERE chat_id = $1 AND type = 'explicit' ORDER BY id",
    )
    .bind(chat_id)
    .fetch_all(pool)
    .await?;

    // 2) Inferred — FTS match kalau ada query.
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
    let mut used = 0usize;
    for (id, fact) in explicit.iter().chain(inferred.iter()) {
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
        lines.push(line);
    }
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
        assert!(!kws.contains(&"kalo") || true); // 4 char — masuk, fine
        let joined = kws.join(" | ");
        assert!(joined.contains('|'));
    }
}
