use crate::config::Config;
use crate::context;
use crate::memory;
use crate::provider::{ApiMessage, AiProvider, ContentBlock};
use crate::shell::ShellCtx;
use crate::soul;
use crate::tools;
use crate::web::WebCtx;
use anyhow::Result;
use sqlx::PgPool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Guard anti-loop: maksimum iterasi tool-calling per turn.
const MAX_TOOL_ITERATIONS: usize = 8;
/// Batas panjang pesan yang dipersist ke history (jaga tabel messages ramping).
const MAX_SAVED_CHARS: usize = 8000;

#[derive(Default)]
pub struct UsageAcc {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub turns: u64,
}

pub struct Agent {
    pub pool: PgPool,
    pub provider: Arc<dyn AiProvider>,
    pub cfg: Config,
    /// State shell access (cwd per chat, pending confirmation, bot handle) — Pilar 9.
    pub shell: Arc<ShellCtx>,
    /// State web access (search provider, HTTP client SSRF-safe, bot handle) — Pilar 10/12.
    pub web: Arc<WebCtx>,
    /// Token usage in-memory per proses (persist menyusul kalau terbukti perlu — ROADMAP).
    pub usage: Arc<Mutex<UsageAcc>>,
    pub started: Instant,
}

impl Agent {
    pub fn new(
        pool: PgPool,
        provider: Arc<dyn AiProvider>,
        cfg: Config,
        shell: Arc<ShellCtx>,
        web: Arc<WebCtx>,
    ) -> Self {
        Self {
            pool,
            provider,
            cfg,
            shell,
            web,
            usage: Arc::new(Mutex::new(UsageAcc::default())),
            started: Instant::now(),
        }
    }

    /// System prompt = soul (Pilar 3) + curated memory (Pilar 5) + waktu sekarang.
    fn build_system_prompt(&self, facts: &str) -> String {
        format!(
            "{}\n\n---\n\n## Memory — fakta tentang owner\n{}\n\n## Waktu sekarang\n{} (UTC) — \
             owner di zona Asia/Jakarta (UTC+7).\n\n## Tools\nKamu punya tools: `create_reminder` \
             (pengingat / tugas terjadwal / rutinitas berulang), `save_memory` (fakta penting \
             owner), `run_command` (shell di VPS — cwd diingat antar panggilan sehingga `cd` \
             efektif; command destruktif otomatis minta approval owner via tombol Telegram), \
             `read_file`/`write_file` (hanya dalam workdir yang diizinkan), `web_search` \
             (info terkini dari web), `fetch_url` (isi halaman sebagai markdown), \
             `generate_image` (buat gambar → dikirim sebagai foto ke owner). Gunakan proaktif \
             tanpa diminta kalau konteksnya jelas. Untuk pekerjaan teknis: baca file yang \
             relevan dulu sebelum mengubah apa pun. Untuk pertanyaan yang butuh info terbaru \
             (versi, harga, berita, error baru): selalu `web_search` dulu — jangan menebak \
             dari pengetahuan lama.",
            soul::load(&self.cfg.soul_path),
            facts,
            chrono::Utc::now().to_rfc3339()
        )
    }

    /// Satu turn percakapan lengkap: simpan pesan → load N-history → call provider →
    /// eksekusi tool calls (loop) → balas. `include_history = false` dipakai untuk
    /// scheduled job (context segar, Pilar 4).
    pub async fn run_turn(&self, chat_id: i64, user_text: &str, include_history: bool) -> Result<String> {
        context::save_message(&self.pool, chat_id, "user", user_text).await?;

        let facts = memory::facts_for_prompt(&self.pool, chat_id).await?;
        let system = self.build_system_prompt(&facts);

        let mut messages: Vec<ApiMessage> = if include_history {
            context::recent_messages(&self.pool, chat_id, self.cfg.n_context)
                .await?
                .into_iter()
                .map(|m| ApiMessage {
                    role: m.role,
                    content: vec![ContentBlock::Text { text: m.content }],
                })
                .collect()
        } else {
            vec![ApiMessage {
                role: "user".into(),
                content: vec![ContentBlock::Text { text: user_text.to_string() }],
            }]
        };

        let tool_defs = tools::definitions();
        let mut collected_text = String::new();

        for i in 0..MAX_TOOL_ITERATIONS {
            let resp = self.provider.chat(&system, &messages, &tool_defs).await?;
            tracing::debug!(stop_reason = %resp.stop_reason, iteration = i, "provider response");
            {
                let mut u = self.usage.lock().unwrap();
                u.input_tokens += resp.usage.input_tokens;
                u.output_tokens += resp.usage.output_tokens;
                u.turns += 1;
            }

            // Ekstrak tool calls & teks dari response block
            let tool_uses: Vec<(String, String, serde_json::Value)> = resp
                .blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, name, input } => {
                        Some((id.clone(), name.clone(), input.clone()))
                    }
                    _ => None,
                })
                .collect();
            let turn_text: String = resp
                .blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");

            // Assistant message (blocks lengkap — termasuk tool_use) masuk conversation
            messages.push(ApiMessage {
                role: "assistant".into(),
                content: resp.blocks.clone(),
            });
            if !turn_text.is_empty() {
                if !collected_text.is_empty() {
                    collected_text.push('\n');
                }
                collected_text.push_str(&turn_text);
            }

            if tool_uses.is_empty() {
                break;
            }

            // Eksekusi semua tool call, kirim hasilnya sebagai tool_result
            let mut results = Vec::new();
            for (id, name, input) in tool_uses {
                let out = match tools::execute(&self.shell, &self.web, chat_id, &name, &input).await {
                    Ok(s) => s,
                    Err(e) => format!("❌ tool error: {:#}", e),
                };
                tracing::info!(tool = %name, chat_id, result = %out, "tool executed");
                results.push(ContentBlock::ToolResult {
                    tool_use_id: id,
                    content: out,
                });
            }
            messages.push(ApiMessage {
                role: "user".into(),
                content: results,
            });

            if i == MAX_TOOL_ITERATIONS - 1 {
                tracing::warn!(chat_id, "tool loop mencapai batas iterasi — dipaksa berhenti");
            }
        }

        if collected_text.trim().is_empty() {
            collected_text = "(tidak ada respons teks dari model)".into();
        }

        let saved: String = collected_text.chars().take(MAX_SAVED_CHARS).collect();
        context::save_message(&self.pool, chat_id, "assistant", &saved).await?;
        Ok(collected_text)
    }
}
