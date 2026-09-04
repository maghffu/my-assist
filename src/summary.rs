//! Session summary (OV-2, adopsi pola session-commit OpenViking): pesan yang
//! jatuh dari context window (`N_CONTEXT`, Pilar 2) TIDAK lagi hilang diam-diam —
//! batch terlama diringkas jadi **rolling summary** per chat (satu baris
//! `session_summaries`), lalu pesan aslinya dihapus (tabel messages tetap ramping).
//!
//! Karakteristik ringkasan = turunan L1 overview OpenViking: konteks + keputusan +
//! hasil, bukan transkrip. Kalau cap tercapai, summary lama + batch baru
//! di-recompress jadi summary baru (self-compacting).
//!
//! Fail-safe: LLM call gagal → TIDAK ada delete; batch di-retry turn berikutnya.
//! Hapus hanya setelah UPSERT summary sukses.

use crate::agent::Agent;
use crate::provider::{ApiMessage, ContentBlock};
use crate::review::call_llm_text;
use anyhow::Result;
use sqlx::PgPool;
use std::sync::OnceLock;
use tokio::sync::Mutex;

/// Batch minimum pesan jatuh-window sebelum diarsipkan — biaya LLM call sepadan.
pub const MIN_ARCHIVE_BATCH: i64 = 10;
/// Cap ringkasan tersimpan (char, boundary char bukan byte).
pub const SUMMARY_MAX_CHARS: usize = 3_000;
/// Cap ringkasan saat di-inject ke system prompt (char).
pub const SUMMARY_INJECT_CAP: usize = 2_500;
/// Cap satu pesan dalam teks batch yang dikirim ke LLM (char).
const BATCH_MSG_CAP: usize = 1_500;
/// Cap total teks batch yang dikirim ke LLM (char).
const BATCH_TEXT_CAP: usize = 20_000;

/// Guard global: satu arsip berjalan pada satu waktu (single-user — tidak perlu
/// per-chat). try_lock gagal → skip turn ini (bukan queue — hindari penumpukan).
static ARCHIVE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Spawn arsip fire-and-forget pasca-turn (dipanggil gateway, Pilar 6 pattern).
pub fn spawn_summary_archive(agent: std::sync::Arc<Agent>, chat_id: i64) {
    tokio::spawn(async move {
        let lock = ARCHIVE_LOCK.get_or_init(|| Mutex::new(()));
        let Ok(_guard) = lock.try_lock() else {
            return; // arsip lain sedang jalan — skip turn ini
        };
        if let Err(e) = archive_once(&agent, chat_id).await {
            // fail-safe: tidak ada delete yang terjadi — batch di-retry turn berikutnya
            tracing::warn!(chat_id, "session summary gagal (batch di-retry turn berikutnya): {e:#}");
        }
    });
}

/// Berapa pesan yang harus diarsipkan turn ini: jumlah jatuh-window kalau sudah
/// ≥ MIN_ARCHIVE_BATCH, selain itu 0. Pure fn (unit-testable).
pub fn overflow_count(count_total: i64, n_context: i64) -> i64 {
    let overflow = count_total - n_context;
    if overflow >= MIN_ARCHIVE_BATCH {
        overflow
    } else {
        0
    }
}

/// Potong di boundary char (bukan byte) — aman untuk emoji/astral char.
pub fn trim_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Susun teks batch percakapan utk LLM: `[role]: content` per baris, per pesan
/// di-cap BATCH_MSG_CAP, total di-cap BATCH_TEXT_CAP (buang dari depan — bagian
/// terbaru batch dipertahankan). Pure fn (unit-testable).
pub fn build_batch_text(batch: &[(i64, String, String)]) -> String {
    let mut lines: Vec<String> = batch
        .iter()
        .map(|(_, role, content)| {
            let who = if role == "assistant" { "assistant" } else { "user" };
            format!("[{who}]: {}", trim_chars(content.trim(), BATCH_MSG_CAP))
        })
        .collect();
    if lines.len() > 1 {
        lines.push(String::new()); // pemisah antar pesan
    }
    let mut text = lines.join("\n");
    if text.chars().count() > BATCH_TEXT_CAP {
        let skip = text.chars().count() - BATCH_TEXT_CAP;
        text = format!("(awal batch dipotong)\n{}", text.chars().skip(skip).collect::<String>());
    }
    text
}

/// Satu siklus arsip: hitung batch → summarize (existing + batch) → UPSERT →
/// DELETE batch. Hapus HANYA setelah UPSERT sukses.
async fn archive_once(agent: &Agent, chat_id: i64) -> Result<()> {
    let n_context = agent.cfg.n_context.max(0);
    let (count_total,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM messages WHERE chat_id = $1")
            .bind(chat_id)
            .fetch_one(&agent.pool)
            .await?;
    let n_archive = overflow_count(count_total, n_context);
    if n_archive == 0 {
        return Ok(());
    }
    // batch = pesan TERLAMAU sebanyak overflow (window hidup N_CONTEXT tetap utuh)
    let batch: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT id, role, content FROM messages WHERE chat_id = $1 ORDER BY id ASC LIMIT $2",
    )
    .bind(chat_id)
    .bind(n_archive)
    .fetch_all(&agent.pool)
    .await?;
    if batch.is_empty() {
        return Ok(());
    }
    let archived_to = batch.last().map(|b| b.0).unwrap_or(0); // batch urut id ASC

    let existing: Option<String> = sqlx::query_scalar("SELECT summary FROM session_summaries WHERE chat_id = $1")
        .bind(chat_id)
        .fetch_optional(&agent.pool)
        .await?;
    let existing_block = existing
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("(belum ada — ini arsip pertama)");

    let system = format!(
        "Kamu modul arsip sesi asisten pribadi. Gabungkan RINGKASAN LAMA dan \
         BATCH PERCAKAPAN BARU menjadi SATU ringkasan konsolidat — distilasi \
         (konteks + keputusan + hasil), bukan transkrip.\n\
         \nAturan:\n\
         1. PERTAHANKAN: fakta penting, keputusan & kesepakatan, nama file/path/command \
         yang dipakai, angka & tanggal, hasil/status task, rencana yang belum selesai.\n\
         2. BUANG: basa-basi, sapaan, langkah antara yang sudah selesai dan tidak \
         relevan lagi, duplikasi dengan isi ringkasan lama.\n\
         3. Format: markdown ringkas, poin per topik, urutan kronologis. Bahasa Indonesia.\n\
         4. Maksimal ±{SUMMARY_MAX_CHARS} karakter. Output HANYA ringkasannya — \
         tanpa pembuka, tanpa penjelasan, tanpa code fence."
    );
    let messages = vec![ApiMessage {
        role: "user".into(),
        content: vec![ContentBlock::Text {
            text: format!(
                "[ringkasan lama]:\n{existing_block}\n\n[batch percakapan baru — pesan \
                 terlama yang jatuh dari context window]:\n{}",
                build_batch_text(&batch)
            ),
        }],
    }];

    let text = call_llm_text(agent, &system, &messages).await?;
    let summary = trim_chars(text.trim(), SUMMARY_MAX_CHARS);
    if summary.is_empty() {
        anyhow::bail!("summary kosong dari LLM");
    }

    // UPSERT dulu (idempoten via PK chat_id) — baru delete batch.
    sqlx::query(
        "INSERT INTO session_summaries (chat_id, summary, archived_to, updated_at) \
         VALUES ($1, $2, $3, now()) \
         ON CONFLICT (chat_id) DO UPDATE \
         SET summary = $2, archived_to = $3, updated_at = now()",
    )
    .bind(chat_id)
    .bind(&summary)
    .bind(archived_to)
    .execute(&agent.pool)
    .await?;
    sqlx::query("DELETE FROM messages WHERE chat_id = $1 AND id <= $2")
        .bind(chat_id)
        .bind(archived_to)
        .execute(&agent.pool)
        .await?;

    tracing::info!(
        chat_id,
        archived = batch.len(),
        summary_chars = summary.chars().count(),
        archived_to,
        "session archived: {} pesan → {} char summary",
        batch.len(),
        summary.chars().count()
    );
    Ok(())
}

/// Ringkasan tersimpan utk chat (summary, archived_to) — di-load per turn utk
/// system prompt (agent.rs). None kalau belum ada arsip.
pub async fn get_summary(pool: &PgPool, chat_id: i64) -> Result<Option<(String, i64)>> {
    let row: Option<(String, i64)> =
        sqlx::query_as("SELECT summary, archived_to FROM session_summaries WHERE chat_id = $1")
            .bind(chat_id)
            .fetch_optional(pool)
            .await?;
    Ok(row)
}

/// Hapus ringkasan — dipakai /new (reset session = reset ringkasan juga).
pub async fn clear_summary(pool: &PgPool, chat_id: i64) -> Result<u64> {
    let res = sqlx::query("DELETE FROM session_summaries WHERE chat_id = $1")
        .bind(chat_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflow_math() {
        // window 20: 25 pesan → 5 jatuh-window, tapi < MIN_BATCH (10) → tidak arsip
        assert_eq!(overflow_count(25, 20), 0);
        // 30 pesan → 10 jatuh-window = tepat minimum → arsip 10
        assert_eq!(overflow_count(30, 20), 10);
        // 47 pesan → 27 jatuh-window → arsip 27 (window 20 tetap utuh)
        assert_eq!(overflow_count(47, 20), 27);
        // window lebih besar dari jumlah pesan → 0
        assert_eq!(overflow_count(15, 20), 0);
        assert_eq!(overflow_count(0, 20), 0);
    }

    #[test]
    fn trim_at_char_boundary() {
        // emoji astral (4 byte/char) — potong per char, bukan byte
        let s = "🎉🎉🎉🎉🎉";
        assert_eq!(trim_chars(s, 3), "🎉🎉🎉");
        // teks biasa
        assert_eq!(trim_chars("abcdef", 10), "abcdef");
        assert_eq!(trim_chars("abcdef", 3), "abc");
        assert_eq!(trim_chars("", 5), "");
    }

    #[test]
    fn batch_text_caps() {
        let batch = vec![
            (1, "user".into(), "halo".into()),
            (2, "assistant".into(), "hai, ada yang bisa dibantu?".into()),
        ];
        let t = build_batch_text(&batch);
        assert!(t.contains("[user]: halo"));
        assert!(t.contains("[assistant]: hai, ada yang bisa dibantu?"));

        // pesan panjang di-cap per pesan
        let long: String = "x".repeat(5_000);
        let t = build_batch_text(&[(1, "user".into(), long.clone())]);
        assert!(t.chars().count() < 5_000, "per-pesan cap berlaku");
        assert!(!t.contains(&"y".repeat(10)));

        // total cap: banyak pesan panjang → dipotong dari depan + marker
        let msgs: Vec<(i64, String, String)> = (0..30)
            .map(|i| (i, "user".into(), format!("msg-{i} {}", "z".repeat(1_400))))
            .collect();
        let t = build_batch_text(&msgs);
        assert!(t.starts_with("(awal batch dipotong)"), "marker muncul: {}", &t[..40]);
        assert!(t.chars().count() <= BATCH_TEXT_CAP + 40);
        // pesan TERBARU (paling relevan) tetap utuh di bagian akhir
        assert!(t.contains("msg-29"), "bagian akhir batch dipertahankan");
        assert!(!t.contains("msg-0"), "bagian paling awal dibuang");
    }
}
