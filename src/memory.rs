use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// Cap karakter fakta EXPLICIT per chat_id — melindungi budget hot tier di
/// system prompt (10k di context.rs). Explicit selalu masuk prompt, jadi ini
/// yang wajib ketat. Pub: dipakai dreaming utk tekanan budget (review.rs).
pub const MAX_EXPLICIT_CHARS: i64 = 9_000;

/// Safety net karakter total (explicit + inferred). Inferred hanya masuk
/// prompt via FTS recall (budget terpisah), jadi boleh bengkak — cap besar
/// ini hanya mencegah DB tumbuh tak terbatas.
const MAX_TOTAL_CHARS: i64 = 100_000;

/// Half-life hotness (hari) — adopsi OpenViking memory_lifecycle.
const HOTNESS_HALF_LIFE_DAYS: f64 = 7.0;

// ============ Memory v3 — kinds + audit trail (adopsi OpenViking OV-3) ============

/// Taxonomy kind (4+general, dipangkas dari 9 types OpenViking — lihat migration 0007).
pub const KINDS: [&str; 5] = ["profile", "preference", "entity", "event", "general"];

/// Validasi kind — baris lama default 'general' sebelum di-reclassify dreaming.
pub fn valid_kind(kind: &str) -> bool {
    KINDS.contains(&kind)
}

/// Label Indonesia utk sub-header recall & /memory.
pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "profile" => "Profil",
        "preference" => "Preferensi",
        "entity" => "Entitas",
        "event" => "Peristiwa",
        _ => "Umum",
    }
}

pub struct MemoryFact {
    pub id: i64,
    pub fact: String,
    pub fact_type: String,
    pub kind: String,
    pub access_count: i64,
    pub accessed_at: DateTime<Utc>,
}

/// Satu baris audit memory_changes (pola memory_diff.json OpenViking).
pub struct MemoryChange {
    pub memory_id: Option<i64>,
    pub action: String,
    pub old_fact: Option<String>,
    pub new_fact: Option<String>,
    pub old_kind: Option<String>,
    pub new_kind: Option<String>,
    pub source: String,
    pub created_at: DateTime<Utc>,
}

/// Tulis baris audit. dipanggil semua jalur tulis setelah mutasi sukses — bukan
/// setelah cap-reject (bukan perubahan). old/new = (fact, type, kind).
async fn audit_change(
    pool: &PgPool,
    chat_id: i64,
    memory_id: Option<i64>,
    action: &str,
    old: Option<(&str, &str, &str)>,
    new: Option<(&str, &str, &str)>,
    source: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO memory_changes \
         (chat_id, memory_id, action, old_fact, new_fact, old_type, new_type, old_kind, new_kind, source) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(chat_id)
    .bind(memory_id)
    .bind(action)
    .bind(old.map(|(f, _, _)| f))
    .bind(new.map(|(f, _, _)| f))
    .bind(old.map(|(_, t, _)| t))
    .bind(new.map(|(_, t, _)| t))
    .bind(old.map(|(_, _, k)| k))
    .bind(new.map(|(_, _, k)| k))
    .bind(source)
    .execute(pool)
    .await?;
    Ok(())
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

/// Bobot hotness pada ranking inferred recall (0 = pure FTS rank, 1 = pure hotness).
const HOTNESS_ALPHA: f64 = 0.3;

/// Skor blend inferred recall: (1-a)*rank_norm + a*hotness_norm.
/// Normalisasi bagi-max (monotonic — urutan relatif tiap komponen terjaga).
pub fn blend_rank_hotness(rank: f64, max_rank: f64, hot: f64, max_hot: f64) -> f64 {
    let rn = if max_rank > 0.0 { rank / max_rank } else { 0.0 };
    let hn = if max_hot > 0.0 { hot / max_hot } else { 0.0 };
    (1.0 - HOTNESS_ALPHA) * rn + HOTNESS_ALPHA * hn
}

/// Tandai fakta barusan ter-inject ke prompt (dipanggil pasca recall).
pub async fn bump_access(pool: &PgPool, ids: &[i64]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "UPDATE memory SET access_count = access_count + 1, accessed_at = now() WHERE id = ANY($1)",
    )
    .bind(ids)
    .execute(pool)
    .await?;
    Ok(())
}

/// Total karakter fakta explicit per chat — tekanan budget dreaming &
/// laporan /dream (before→after). Query sama dgn guard cap di save_fact.
pub async fn explicit_chars(pool: &PgPool, chat_id: i64) -> Result<i64> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(LENGTH(fact)), 0) FROM memory WHERE chat_id = $1 AND type = 'explicit'",
    )
    .bind(chat_id)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

pub async fn save_fact(
    pool: &PgPool,
    chat_id: i64,
    fact: &str,
    fact_type: &str,
    kind: &str,
    source: &str,
) -> Result<String> {
    let (used,): (i64,) =
        sqlx::query_as("SELECT COALESCE(SUM(LENGTH(fact)), 0) FROM memory WHERE chat_id = $1")
            .bind(chat_id)
            .fetch_one(pool)
            .await?;
    if fact_type == "explicit" {
        let used_explicit = explicit_chars(pool, chat_id).await?;
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
    // defensive: kind invalid (mis. output LLM liar) → general, bukan error
    let kind = if valid_kind(kind) { kind } else { "general" };
    let row: (i64,) =
        sqlx::query_as("INSERT INTO memory (chat_id, fact, type, kind) VALUES ($1, $2, $3, $4) RETURNING id")
            .bind(chat_id)
            .bind(fact)
            .bind(fact_type)
            .bind(kind)
            .fetch_one(pool)
            .await?;
    audit_change(
        pool,
        chat_id,
        Some(row.0),
        "insert",
        None,
        Some((fact, fact_type, kind)),
        source,
    )
    .await?;
    Ok(format!("✅ Memory tersimpan [{}|{}]: {}", fact_type, kind, fact))
}

pub async fn list_facts(pool: &PgPool, chat_id: i64, limit: i64) -> Result<Vec<MemoryFact>> {
    // access_count::int8 — kolom DB INT4 (migration 0005) vs Rust i64; tanpa cast,
    // sqlx decode gagal dan MEMATIKAN setiap turn (bug deploy 7f14b38b).
    let rows = sqlx::query_as::<_, (i64, String, String, String, i64, DateTime<Utc>)>(
        "SELECT id, fact, type, kind, access_count::int8, accessed_at FROM memory \
         WHERE chat_id = $1 ORDER BY id DESC LIMIT $2",
    )
    .bind(chat_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(id, fact, fact_type, kind, access_count, accessed_at)| MemoryFact {
                id,
                fact,
                fact_type,
                kind,
                access_count,
                accessed_at,
            },
        )
        .collect())
}

/// Baris audit terbaru per chat — reader utk /memory log (OV-3).
pub async fn list_changes(
    pool: &PgPool,
    chat_id: i64,
    limit: i64,
) -> Result<Vec<MemoryChange>> {
    let rows = sqlx::query_as::<_, (
        Option<i64>,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        DateTime<Utc>,
    )>(
        "SELECT memory_id, action, old_fact, new_fact, old_kind, new_kind, source, created_at \
         FROM memory_changes WHERE chat_id = $1 ORDER BY id DESC LIMIT $2",
    )
    .bind(chat_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(memory_id, action, old_fact, new_fact, old_kind, new_kind, source, created_at)| {
                MemoryChange {
                    memory_id,
                    action,
                    old_fact,
                    new_fact,
                    old_kind,
                    new_kind,
                    source,
                    created_at,
                }
            },
        )
        .collect())
}

pub async fn delete_fact(pool: &PgPool, chat_id: i64, id: i64, source: &str) -> Result<bool> {
    // snapshot dulu utk audit (memory_id jadi NULL setelah baris hilang)
    let old: Option<(String, String, String)> =
        sqlx::query_as("SELECT fact, type, kind FROM memory WHERE id = $1 AND chat_id = $2")
            .bind(id)
            .bind(chat_id)
            .fetch_optional(pool)
            .await?;
    let res = sqlx::query("DELETE FROM memory WHERE id = $1 AND chat_id = $2")
        .bind(id)
        .bind(chat_id)
        .execute(pool)
        .await?;
    if res.rows_affected() > 0 {
        if let Some((f, t, k)) = old {
            audit_change(pool, chat_id, Some(id), "delete", Some((f.as_str(), t.as_str(), k.as_str())), None, source).await?;
        }
        Ok(true)
    } else {
        Ok(false)
    }
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
/// Audit `update` (old_fact → new_fact) — pola memory_diff OpenViking.
pub async fn update_fact(
    pool: &PgPool,
    chat_id: i64,
    id: i64,
    fact: &str,
    source: &str,
) -> Result<bool> {
    let old: Option<(String, String, String)> =
        sqlx::query_as("SELECT fact, type, kind FROM memory WHERE id = $1 AND chat_id = $2")
            .bind(id)
            .bind(chat_id)
            .fetch_optional(pool)
            .await?;
    let res = sqlx::query(
        "UPDATE memory SET fact = $3, accessed_at = now() WHERE id = $1 AND chat_id = $2",
    )
    .bind(id)
    .bind(chat_id)
    .bind(fact)
    .execute(pool)
    .await?;
    if res.rows_affected() > 0 {
        if let Some((old_fact, t, k)) = old {
            audit_change(
                pool,
                chat_id,
                Some(id),
                "update",
                Some((old_fact.as_str(), t.as_str(), k.as_str())),
                Some((fact, t.as_str(), k.as_str())),
                source,
            )
            .await?;
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Ubah tipe fakta (dreaming: upgrade inferred → explicit, Pilar 6). Audit `retype`.
pub async fn set_fact_type(
    pool: &PgPool,
    chat_id: i64,
    id: i64,
    fact_type: &str,
    source: &str,
) -> Result<bool> {
    if !matches!(fact_type, "explicit" | "inferred") {
        anyhow::bail!("fact_type tidak valid: {fact_type}");
    }
    let old: Option<(String, String, String)> =
        sqlx::query_as("SELECT fact, type, kind FROM memory WHERE id = $1 AND chat_id = $2")
            .bind(id)
            .bind(chat_id)
            .fetch_optional(pool)
            .await?;
    let res = sqlx::query("UPDATE memory SET type = $3 WHERE id = $1 AND chat_id = $2")
        .bind(id)
        .bind(chat_id)
        .bind(fact_type)
        .execute(pool)
        .await?;
    if res.rows_affected() > 0 {
        if let Some((f, old_type, k)) = old {
            audit_change(
                pool,
                chat_id,
                Some(id),
                "retype",
                Some((f.as_str(), old_type.as_str(), k.as_str())),
                Some((f.as_str(), fact_type, k.as_str())),
                source,
            )
            .await?;
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Ubah kind fakta (dreaming reclassify — sekaligus mekanisme backfill baris
/// `general` lama, bertahap per cycle). Audit `reclassify`.
pub async fn set_kind(
    pool: &PgPool,
    chat_id: i64,
    id: i64,
    kind: &str,
    source: &str,
) -> Result<bool> {
    if !valid_kind(kind) {
        anyhow::bail!("kind tidak valid: {kind} (harus salah satu dari {KINDS:?})");
    }
    let old: Option<(String, String, String)> =
        sqlx::query_as("SELECT fact, type, kind FROM memory WHERE id = $1 AND chat_id = $2")
            .bind(id)
            .bind(chat_id)
            .fetch_optional(pool)
            .await?;
    let res = sqlx::query("UPDATE memory SET kind = $3 WHERE id = $1 AND chat_id = $2")
        .bind(id)
        .bind(chat_id)
        .bind(kind)
        .execute(pool)
        .await?;
    if res.rows_affected() > 0 {
        if let Some((f, t, old_kind)) = old {
            audit_change(
                pool,
                chat_id,
                Some(id),
                "reclassify",
                Some((f.as_str(), t.as_str(), old_kind.as_str())),
                Some((f.as_str(), t.as_str(), kind)),
                source,
            )
            .await?;
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Daftar fakta untuk disuntik ke system prompt (urut hotness — paling sering
/// & terakhir dipakai di atas, biar budget cut kena yang cold duluan).
#[allow(dead_code)] // dipertahankan dari memory v1; jalur aktif = recall_facts
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
        .map(|f| format!("- {} [{}|{}]", f.fact, f.fact_type, f.kind))
        .collect::<Vec<_>>()
        .join("\n"))
}

// ============ Memory v2 — recall-based injection (adopsi hermes-agent) ============

/// Budget karakter maksimal yang di-inject ke system prompt (recall).
const RECALL_BUDGET_CHARS: usize = 10_000;

/// Recall selectif: explicit selalu masuk (urut hotness); inferred hanya kalau
/// match FTS terhadap kata kunci pesan user, ATAU jika query kosong (scheduled
/// job). Output dikelompokkan per kind dengan sub-header (OV-3), sudah didedup
/// + dipotong budget. IDs yang ter-inject di-bump. Budget/dedupe/hotness logic
/// TIDAK berubah dari v2.
pub async fn recall_facts(pool: &PgPool, chat_id: i64, query: &str) -> Result<String> {
    // 1) Explicit — selalu (core identity owner, gak boleh hilang karena keyword miss).
    let explicit_rows = sqlx::query_as::<_, (i64, String, String, i64, DateTime<Utc>)>(
        "SELECT id, fact, kind, access_count::int8, accessed_at FROM memory \
         WHERE chat_id = $1 AND type = 'explicit' ORDER BY id",
    )
    .bind(chat_id)
    .fetch_all(pool)
    .await?;
    let now = Utc::now();
    let mut explicit_rows: Vec<(i64, String, String, i64, DateTime<Utc>)> = explicit_rows;
    explicit_rows.sort_by(|a, b| {
        hotness(b.3, b.4, now)
            .partial_cmp(&hotness(a.3, a.4, now))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let explicit: Vec<(i64, String, String)> = explicit_rows
        .into_iter()
        .map(|(id, fact, kind, ..)| (id, fact, kind))
        .collect();

    // 2) Inferred — FTS match kalau ada query, ranking blend rank+hotness.
    //    AND semua kata pesan user terlalu ketat (pesan panjang = pasti 0 hasil),
    //    jadi: ekstrak keywords (>=4 char, alnum), gabung OR, threshold.
    //    Urutan final: (1-a)*rank_norm + a*hotness_norm (adopsi OpenViking blend).
    let mut inferred: Vec<(i64, String, String)> = Vec::new();
    let kws: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 4)
        .map(String::from)
        .take(12)
        .collect();
    if !kws.is_empty() {
        let or_query = kws.join(" | ");
        let rows = sqlx::query_as::<_, (i64, String, String, i64, DateTime<Utc>, f64)>(
            r#"SELECT id, fact, kind, access_count::int8, accessed_at,
                      ts_rank(search_vector, to_tsquery('simple', $2))::float8 AS rank
               FROM memory
               WHERE chat_id = $1 AND type = 'inferred'
                 AND search_vector @@ to_tsquery('simple', $2)
                 AND ts_rank(search_vector, to_tsquery('simple', $2)) > 0.01
               ORDER BY rank DESC
               LIMIT 30"#,
        )
        .bind(chat_id)
        .bind(&or_query)
        .fetch_all(pool)
        .await
        .unwrap_or_default(); // FTS gagal (misal syntax aneh) -> inferred kosong, bukan error
                              // Blend (1-a)*rank + a*hotness; normalisasi bagi-max (monotonic per komponen).
        if !rows.is_empty() {
            let max_rank = rows.iter().map(|r| r.5).fold(0.0_f64, f64::max);
            let hots: Vec<f64> = rows.iter().map(|r| hotness(r.3, r.4, now)).collect();
            let max_hot = hots.iter().copied().fold(0.0_f64, f64::max);
            let mut blended: Vec<(f64, i64, String, String)> = rows
                .iter()
                .zip(hots.iter())
                .map(|(r, h)| {
                    (
                        blend_rank_hotness(r.5, max_rank, *h, max_hot),
                        r.0,
                        r.1.clone(),
                        r.2.clone(),
                    )
                })
                .collect();
            blended.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            inferred = blended
                .into_iter()
                .map(|(_, id, fact, kind)| (id, fact, kind))
                .collect();
        }
    }

    // 3) Gabung + dedup (substring-case-insensitive; pola duplikat semantik "backup v1/v2").
    let mut seen: Vec<String> = Vec::new();
    let mut selected: Vec<(i64, String, String)> = Vec::new(); // (id, fact, kind)
    let mut injected_ids: Vec<i64> = Vec::new();
    let mut used = 0usize;
    for (id, fact, kind) in explicit.iter().chain(inferred.iter()) {
        let low = fact.to_lowercase();
        if seen
            .iter()
            .any(|s| low.contains(s.as_str()) || s.contains(&low))
        {
            continue;
        }
        let line = format!("- {} [id:{}]", fact, id);
        if used + line.len() > RECALL_BUDGET_CHARS {
            break;
        }
        used += line.len();
        seen.push(low);
        injected_ids.push(*id);
        selected.push((*id, fact.clone(), kind.clone()));
    }
    bump_access(pool, &injected_ids).await?;
    if selected.is_empty() {
        return Ok("(belum ada fakta tersimpan)".into());
    }
    Ok(group_by_kind(&selected))
}

/// Kelompokkan fakta terpilih per kind dengan sub-header (OV-3) — hanya header
/// yang isinya non-kosong, urutan tetap taxonomy. Pure fn (unit-testable).
pub fn group_by_kind(selected: &[(i64, String, String)]) -> String {
    let mut out = String::new();
    for kind in KINDS {
        let group: Vec<&(i64, String, String)> =
            selected.iter().filter(|(_, _, k)| k == kind).collect();
        if group.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("### {}\n", kind_label(kind)));
        for (id, fact, _) in group {
            out.push_str(&format!("- {} [id:{}]\n", fact, id));
        }
    }
    out.trim_end().to_string()
}

/// Filter entri transien/trivial — jangan sampai intermediate step masuk long-term memory.
pub fn is_transient_fact(fact: &str) -> bool {
    let f = fact.trim();
    let low = f.to_lowercase();
    let transient_markers = [
        "owner setuju",
        "disetujui",
        "approved",
        "kandidat delete",
        "kandidat prune",
        "menunggu konfirmasi",
        "menunggu keputusan",
        "menunggu owner",
        "akan dieksekusi",
        "sedang dikerjakan",
        "sebentar",
        "in progress",
        "todo:",
        "queue ke antrian",
        "masuk antrian",
        "langkah berikutnya",
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
pub async fn find_near_duplicate(pool: &PgPool, chat_id: i64, fact: &str) -> Result<Option<i64>> {
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
    fn kind_validation() {
        assert!(valid_kind("profile"));
        assert!(valid_kind("preference"));
        assert!(valid_kind("entity"));
        assert!(valid_kind("event"));
        assert!(valid_kind("general"));
        assert!(!valid_kind("identity")); // bukan taxonomy kita (sudah dipegang soul.md)
        assert!(!valid_kind(""));
        assert_eq!(kind_label("event"), "Peristiwa");
        assert_eq!(kind_label("unknown"), "Umum");
    }

    #[test]
    fn recall_grouping_by_kind() {
        let selected = vec![
            (1, "Owner deploy suka jam malam".into(), "preference".into()),
            (2, "Owner tinggal di REDACTED-CITY".into(), "profile".into()),
            (3, "VPS utama 2 vCPU di Jakarta".into(), "entity".into()),
            (4, "fakta lama tanpa kind".into(), "general".into()),
        ];
        let out = group_by_kind(&selected);
        // urutan tetap taxonomy, hanya header non-kosong (tidak ada '### Entitas'? ada —
        // semua kind terisi di sini; event kosong → header event TIDAK muncul)
        let idx_profile = out.find("### Profil").unwrap();
        let idx_pref = out.find("### Preferensi").unwrap();
        let idx_entity = out.find("### Entitas").unwrap();
        let idx_general = out.find("### Umum").unwrap();
        assert!(idx_profile < idx_pref && idx_pref < idx_entity && idx_entity < idx_general);
        assert!(!out.contains("### Peristiwa"), "kind kosong tidak punya header: {out}");
        assert!(out.contains("- Owner deploy suka jam malam [id:1]"));

        // satu kind saja → satu header, tanpa baris kosong
        let one = group_by_kind(&[(9, "x".into(), "event".into())]);
        assert_eq!(one, "### Peristiwa\n- x [id:9]");
    }

    #[test]
    fn blend_rank_hotness_weights_and_guards() {
        // a=0.3: rank dominan (bobot 0.7), hotness booster (bobot 0.3).
        let full_rank = blend_rank_hotness(1.0, 1.0, 0.0, 1.0);
        let full_hot = blend_rank_hotness(0.0, 1.0, 1.0, 1.0);
        assert!((full_rank - 0.7).abs() < 1e-9);
        assert!((full_hot - 0.3).abs() < 1e-9);
        assert!(full_rank > full_hot);
        // Monotonik di kedua komponen.
        assert!(blend_rank_hotness(0.8, 1.0, 0.5, 1.0) > blend_rank_hotness(0.4, 1.0, 0.5, 1.0));
        assert!(blend_rank_hotness(0.5, 1.0, 0.9, 1.0) > blend_rank_hotness(0.5, 1.0, 0.1, 1.0));
        // Guard max=0: hasil 0.0, bukan NaN.
        assert!(blend_rank_hotness(0.5, 0.0, 0.5, 0.0) == 0.0);
    }

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
        assert!(!is_transient_fact(
            "owner lari sore biasanya jam 17:00 di Stadion REDACTED-CITY"
        ));
        assert!(!is_transient_fact(
            "email SMTP pakai example.com port 465 SSL"
        ));
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
