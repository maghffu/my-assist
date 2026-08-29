use anyhow::Result;
use sqlx::PgPool;

/// Cap karakter total per chat_id (Pilar 5 — mencegah system prompt membengkak).
const MAX_MEMORY_CHARS: i64 = 20_000;

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
    if used + fact.len() as i64 > MAX_MEMORY_CHARS {
        return Ok(format!(
            "⚠️ Memory cap tercapai ({} karakter). Fakta tidak disimpan — \
             minta owner bersihkan memory lama via /memory del <id>.",
            MAX_MEMORY_CHARS
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
