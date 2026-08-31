use anyhow::Result;
use sqlx::PgPool;

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

pub async fn save_message(pool: &PgPool, chat_id: i64, role: &str, content: &str) -> Result<()> {
    sqlx::query("INSERT INTO messages (chat_id, role, content) VALUES ($1, $2, $3)")
        .bind(chat_id)
        .bind(role)
        .bind(content)
        .execute(pool)
        .await?;
    Ok(())
}

/// N pesan terakhir per chat_id, urut kronologis (Pilar 2 — jangan full history).
pub async fn recent_messages(
    pool: &PgPool,
    chat_id: i64,
    limit: i64,
) -> Result<Vec<ChatMessage>> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT role, content FROM (
            SELECT role, content, created_at, id FROM messages
            WHERE chat_id = $1
            ORDER BY created_at DESC, id DESC
            LIMIT $2
        ) sub ORDER BY created_at ASC, id ASC",
    )
    .bind(chat_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(role, content)| ChatMessage { role, content })
        .collect())
}

/// Hapus seluruh riwayat percakapan satu chat (command /new — reset session).
/// Memory & skills = tabel berbeda, tidak tersentuh (identitas agent tetap).
pub async fn clear_messages(pool: &PgPool, chat_id: i64) -> Result<u64> {
    let res = sqlx::query("DELETE FROM messages WHERE chat_id = $1")
        .bind(chat_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}
