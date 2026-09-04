use crate::config::Config;
use crate::context;
use crate::memory;
use crate::notify::Notifier;
use crate::provider::{ApiMessage, AiProvider, ContentBlock};
use crate::shell::ShellCtx;
use crate::soul;
use crate::tools;
use crate::web::WebCtx;
use anyhow::Result;
use sqlx::PgPool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Guard anti-loop: maksimum iterasi tool-calling per turn. 8 terbukti kurang
/// untuk task multi-langkah (uninstall package = stop/remove/verify, dst.) —
/// agent berhenti diam-diam padahal tugas belum selesai. 16→50 (31 Agu 2026):
/// task build+deploy+verify panjang (menunggu cargo build via poll) butuh >16
/// langkah — terpotong memaksa fragmentasi turn (owner harus bilang "lanjut").
/// Tradeoff: worst-case token spend per turn naik ~3x — diterima karena self-harm
/// guard (shell.rs) sudah memblokir jalur bunuh-diri yang dulu muncul di turn panjang.
/// CATATAN: limit ini BUKAN penyebab insiden self-kill 30-31 Agu (bot mati di
/// tool call ke-4, jauh dari batas apapun) — jangan naikkan lagi demi "mencegah
/// crash"; untuk itu ada guard + Restart=always.
const MAX_TOOL_ITERATIONS: usize = 50;
/// Batas panjang pesan yang dipersist ke history (jaga tabel messages ramping).
const MAX_SAVED_CHARS: usize = 8000;

#[derive(Default)]
pub struct UsageAcc {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub turns: u64,
}

/// Breakdown konteks per turn (OV-4, adopsi observable retrieval OpenViking):
/// apa saja yang masuk prompt — biar tuning recall/injection tidak guesswork.
#[derive(Default, Clone, Debug)]
pub struct ContextTrace {
    pub memory_facts: usize,
    pub memory_chars: usize,
    pub skills_listed: usize,
    pub skills_injected: usize,
    pub summary_chars: usize,
    pub history_msgs: usize,
    pub history_chars: usize,
    pub system_chars: usize,
}

impl ContextTrace {
    /// Satu baris ringkas utk /status (pure fn — unit-testable).
    pub fn fmt_summary(&self) -> String {
        format!(
            "memory {} fakta ({} char) | skills {}/{} di-inject | summary {} char | history {} pesan ({} char) | system {} char",
            self.memory_facts,
            self.memory_chars,
            self.skills_injected,
            self.skills_listed,
            self.summary_chars,
            self.history_msgs,
            self.history_chars,
            self.system_chars
        )
    }
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
    /// Context trace TERAKHIR saja, in-memory (OV-4) — dibaca /status.
    pub last_trace: Arc<Mutex<Option<ContextTrace>>>,
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
            last_trace: Arc::new(Mutex::new(None)),
            started: Instant::now(),
        }
    }

    /// System prompt = soul (Pilar 3) + riwayat sesi ringkas (OV-2) + curated memory
    /// (Pilar 5) + skills (Pilar 11) + waktu sekarang. Skill yang cocok keyword dengan
    /// pesan (nama + L0 description) dimuat penuh (skills.rs). `summary_section` kosong
    /// kalau belum ada arsip sesi.
    fn build_system_prompt(
        &self,
        facts: &str,
        user_text: &str,
        summary_section: &str,
    ) -> (String, crate::skills::SkillsPromptStats) {
        let (skills_section, skills_stats) =
            crate::skills::section_for_prompt(std::path::Path::new(&self.cfg.skills_dir), user_text);
        let history_part = if summary_section.is_empty() {
            String::new()
        } else {
            format!("\n\n## Riwayat percakapan sebelumnya (ringkasan)\n{summary_section}")
        };
        (
            format!(
                "{}\n\n---\n{}\n\n## Memory — fakta tentang owner\n{}\n\n## Skills — pengetahuan prosedural\n{}\n\n## Waktu sekarang\n{} (UTC) — \
             owner di zona Asia/Jakarta (UTC+7).\n\n## Tools\nKamu punya tools: `create_reminder` \
             (pengingat / tugas terjadwal / rutinitas berulang), `save_memory` (fakta penting \
             owner), `run_command` (shell di VPS — cwd diingat antar panggilan sehingga `cd` \
             efektif; command destruktif otomatis minta approval owner via tombol Telegram — \
             kalau owner approve, lanjutkan tugasnya; kalau ditolak/timeout, laporkan ke owner \
             dan jangan ulangi command yang sama sendiri), \
             `read_file`/`write_file` (hanya dalam workdir yang diizinkan), `web_search` \
             (info terkini dari web), `fetch_url` (isi halaman sebagai markdown), \
             `generate_image` (buat gambar → dikirim sebagai foto ke owner), `save_skill` \
             (simpan prosedur non-trivial yang baru dikuasai — lihat aturannya di deskripsi \
             tool). Gunakan proaktif tanpa diminta kalau konteksnya jelas. Untuk pekerjaan \
             teknis: baca file yang relevan dulu sebelum mengubah apa pun. Untuk pertanyaan \
             yang butuh info terbaru (versi, harga, berita, error baru): selalu `web_search` \
             dulu — jangan menebak dari pengetahuan lama. Setelah menyelesaikan masalah teknis \
             non-trivial: simpan prosedurnya dengan `save_skill` (sertakan `description` satu \
kalimat). Saat `save_memory`, pilih `kind` yang tepat: profile (identitas dasar), \
preference (preferensi/kebiasaan), entity (objek/proyek berulang), event (peristiwa \
bertanggal), general (kalau ragu).\n\n## Cara bekerja (WAJIB)\n1. \
             Tugas manajemen sistem yang sah (install/uninstall package, restart service, edit \
             config, debug error) KERJAKAN LANGSUNG dengan tools — jangan menolak dengan saran \
             generik. Command berisiko otomatis diminta approval owner, jadi tidak perlu ragu.\n2. \
             Kalau satu langkah gagal (error, permission, timeout, ditolak owner), jelaskan \
             penyebabnya dari pesan error dan usulkan langkah alternatif — jangan berhenti diam.\n3. \
             Di jawaban akhir selalu ringkas: apa yang sudah dilakukan, hasilnya, dan langkah \
             berikutnya yang disarankan.\n4. Kerjakan tugas multi-langkah sampai tuntas — pakai \
             hasil tiap tool sebagai input langkah berikutnya. Kalau batas langkah tercapai, \
             laporkan status terakhir dan minta owner bilang \"lanjut\".",
            soul::load(&self.cfg.soul_path),
            history_part,
            facts,
            skills_section,
            chrono::Utc::now().to_rfc3339()
            ),
            skills_stats,
        )
    }

    /// Satu turn percakapan lengkap: simpan pesan → load N-history → call provider →
    /// eksekusi tool calls (loop, tiap langkah dilaporkan via Notifier) → balas.
    /// `include_history = false` dipakai untuk scheduled job (context segar, Pilar 4).
    pub async fn run_turn(
        &self,
        chat_id: i64,
        user_text: &str,
        include_history: bool,
        notify: Option<&Notifier>,
    ) -> Result<String> {
        context::save_message(&self.pool, chat_id, "user", user_text).await?;

        // Memory v2: recall selectif — explicit selalu, inferred hanya FTS match pesan user.
        let facts = memory::recall_facts(&self.pool, chat_id, user_text).await?;
        // Session summary (OV-2): ringkasan pesan yang sudah jatuh dari window —
        // konteks stabil hasil distilasi. Ikut untuk scheduled job juga (include_history=
        // false): satu-satunya cara job tahu progres terakhir (trade-off token dicatat).
        let summary_section = match crate::summary::get_summary(&self.pool, chat_id).await? {
            Some((s, _)) if !s.trim().is_empty() => {
                let capped = crate::summary::trim_chars(s.trim(), crate::summary::SUMMARY_INJECT_CAP);
                capped.trim_end().to_string()
            }
            _ => String::new(),
        };
        let system_parts = self.build_system_prompt(&facts, user_text, &summary_section);
        let system = system_parts.0;
        let skills_stats = system_parts.1;

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

        // Context trace (OV-4): breakdown konteks yang dikirim turn ini — data sudah
        // ada di titik ini, tinggal dihitung. In-memory (trace terakhir) + log per turn.
        {
            let trace = ContextTrace {
                memory_facts: facts.lines().filter(|l| l.trim_start().starts_with("- ")).count(),
                memory_chars: facts.chars().count(),
                skills_listed: skills_stats.listed,
                skills_injected: skills_stats.injected,
                summary_chars: summary_section.chars().count(),
                history_msgs: messages.len(),
                history_chars: messages
                    .iter()
                    .map(|m| {
                        m.content
                            .iter()
                            .map(|b| match b {
                                ContentBlock::Text { text } => text.chars().count(),
                                _ => 0,
                            })
                            .sum::<usize>()
                    })
                    .sum::<usize>(),
                system_chars: system.chars().count(),
            };
            tracing::info!(
                chat_id,
                memory_facts = trace.memory_facts,
                memory_chars = trace.memory_chars,
                skills_listed = trace.skills_listed,
                skills_injected = trace.skills_injected,
                summary_chars = trace.summary_chars,
                history_msgs = trace.history_msgs,
                history_chars = trace.history_chars,
                system_chars = trace.system_chars,
                "context trace"
            );
            let mut t = self.last_trace.lock().unwrap();
            *t = Some(trace);
        }

        let mut collected_text = String::new();
        let mut last_stop = String::from("end_turn");

        for i in 0..MAX_TOOL_ITERATIONS {
            // Progres mulai iterasi ke-2 — iterasi pertama ditutupi typing indicator,
            // jadi chat sederhana tanpa tool tetap bersih tanpa pesan status.
            if i > 0 {
                if let Some(n) = notify {
                    n.notify(format!(
                        "🧠 memproses hasil tool… (langkah {}/{})",
                        i + 1,
                        MAX_TOOL_ITERATIONS
                    ));
                }
            }

            let resp = self.provider.chat(&system, &messages, &tool_defs).await?;
            last_stop = resp.stop_reason.clone();
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

            // Eksekusi semua tool call, kirim hasilnya sebagai tool_result. Tiap
            // langkah dilaporkan ke Telegram (pesan progres live) — owner tidak lagi
            // menghadap layar diam selama agent bekerja.
            let mut results = Vec::new();
            for (id, name, input) in tool_uses {
                if let Some(n) = notify {
                    let args = short_input(&input);
                    n.notify(format!(
                        "🔧 {name}{} …",
                        if args.is_empty() { String::new() } else { format!(": {args}") }
                    ));
                }
                let t0 = Instant::now();
                let out = match tools::execute(&self.shell, &self.web, chat_id, &name, &input).await {
                    Ok(s) => {
                        if let Some(n) = notify {
                            n.notify(format!(
                                "✅ {name} selesai ({:.1}s) — lanjut…",
                                t0.elapsed().as_secs_f64()
                            ));
                        }
                        s
                    }
                    Err(e) => {
                        tracing::warn!(tool = %name, chat_id, "tool gagal: {e:#}");
                        if let Some(n) = notify {
                            n.notify(format!(
                                "⚠️ {name} gagal ({:.1}s) — dilaporkan ke model",
                                t0.elapsed().as_secs_f64()
                            ));
                        }
                        format!("❌ tool error: {:#}", e)
                    }
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
                if let Some(n) = notify {
                    n.notify(format!("⚠️ batas {} langkah tool tercapai", MAX_TOOL_ITERATIONS));
                }
            }
        }

        // Jangan pernah balas kosong/diam — setiap kondisi berhenti punya kabar bagi
        // owner (keluhan "ngomong sama tembok").
        if collected_text.trim().is_empty() {
            collected_text = match last_stop.as_str() {
                "tool_use" => format!(
                    "⚠️ Aku berhenti karena mencapai batas {} langkah tool per turn. Status \
                     terakhir sudah diproses — bilang \"lanjut\" kalau tugasnya belum selesai.",
                    MAX_TOOL_ITERATIONS
                ),
                _ => "⚠️ Model tidak mengirim teks balasan — coba kirim ulang pesanmu.".into(),
            };
        } else if last_stop == "max_tokens" {
            collected_text.push_str("\n\n⚠️ (balasan terpotong — batas token model tercapai)");
        }

        let saved: String = collected_text.chars().take(MAX_SAVED_CHARS).collect();
        context::save_message(&self.pool, chat_id, "assistant", &saved).await?;
        Ok(collected_text)
    }
}

/// Ringkasan satu-baris input tool untuk pesan progres
/// (mis. `run_command: npm uninstall -g opencode`).
fn short_input(input: &serde_json::Value) -> String {
    const KEYS: &[&str] = &["command", "query", "url", "path", "prompt", "fact", "message", "name"];
    for k in KEYS {
        if let Some(s) = input.get(*k).and_then(|v| v.as_str()) {
            let one_line: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
            return one_line.chars().take(90).collect();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_trace_summary_line() {
        let tr = ContextTrace {
            memory_facts: 12,
            memory_chars: 8_100,
            skills_listed: 5,
            skills_injected: 1,
            summary_chars: 2_400,
            history_msgs: 20,
            history_chars: 7_200,
            system_chars: 18_000,
        };
        let s = tr.fmt_summary();
        assert!(s.contains("memory 12 fakta (8100 char)"), "{s}");
        assert!(s.contains("skills 1/5 di-inject"), "{s}");
        assert!(s.contains("summary 2400 char"), "{s}");
        assert!(s.contains("history 20 pesan (7200 char)"), "{s}");
        assert!(s.contains("system 18000 char"), "{s}");
    }
}
