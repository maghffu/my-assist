//! Self-improvement loop (Pilar 6, ROADMAP Fase 6) — BUKAN fine-tuning.
//! Model tetap statis; yang "improve" adalah ekstraksi & kurasi memory.
//!
//! Dua proses, keduanya via LLM call internal tanpa tools (JSON output):
//! 1. **Background review pasca-turn** — `tokio::spawn` fire-and-forget setelah
//!    tiap turn owner (via gateway): analisis pertukaran terakhir, ekstrak fakta
//!    `[explicit]` / `[inferred]`, anti-duplikat dengan memory existing.
//! 2. **Dreaming cycle** — berkala (mingguan, plus `/dream` manual): review
//!    SELURUH memory (drop/rewrite/upgrade) dan SELURUH skills (delete/rewrite/merge).
//!
//! Fail-safe: parse gagal / aksi tidak valid → skip, memory existing tidak disentuh.

use crate::agent::Agent;
use crate::memory;
use crate::provider::{ApiMessage, ContentBlock};
use crate::skills;
use anyhow::{bail, Context, Result};
use serde_json::Value;

/// Pertukaran lebih pendek dari ini di-skip (smalltalk/ok/makasih — hemat API call).
const MIN_EXCHANGE_CHARS: usize = 120;
/// Maksimum fakta baru per review (prompt juga menyebut ini).
const MAX_FACTS_PER_REVIEW: usize = 3;
/// Panjang maksimum satu fakta.
const MAX_FACT_CHARS: usize = 300;
/// Batas aksi dreaming per chat (fail-safe kalau LLM lepas kendali).
const MAX_DREAM_ACTIONS: usize = 20;

// ── Post-turn background review ────────────────────────────────────────────────

/// Spawn review fire-and-forget — TIDAK menambah latency respons owner (Pilar 6).
pub fn spawn_post_turn_review(agent: std::sync::Arc<Agent>, chat_id: i64, user_text: String, reply: String) {
    tokio::spawn(async move {
        if let Err(e) = post_turn_review(&agent, chat_id, &user_text, &reply).await {
            tracing::warn!(chat_id, "background review gagal (non-fatal): {e:#}");
        }
    });
}

async fn post_turn_review(agent: &Agent, chat_id: i64, user_text: &str, reply: &str) -> Result<()> {
    if user_text.chars().count() + reply.chars().count() < MIN_EXCHANGE_CHARS {
        return Ok(());
    }
    let existing = memory::list_facts(&agent.pool, chat_id, 200).await?;
    let existing_block = if existing.is_empty() {
        "(belum ada)".into()
    } else {
        existing
            .iter()
            .map(|f| format!("- {} [{}]", f.fact, f.fact_type))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let system = format!(
        "Kamu modul ekstraksi memori asisten pribadi. Analisis SATU pertukaran pesan terakhir \
         dan ekstrak fakta jangka panjang yang berguna LINTAS SESI tentang owner.\n\
         \nAturan:\n\
         1. Hanya fakta STABIL: preferensi, kebiasaan, proyek yang dikerjakan, jadwal rutin, \
         lingkungan teknis (VPS, domain, stack), hubungan. BUKAN: topik obrolan sesaat, \
         permintaan sekali jalan, info basi (cuaca/berita/harga hari ini).\n\
         2. Dua level — tandai jujur:\n\
         - \"explicit\": owner menyebutnya langsung (\"aku deploy-nya suka jam malam\")\n\
         - \"inferred\": kesimpulanmu dari konteks (\"owner mengelola VPS sendiri\") — dugaan, \
         jangan over-claim\n\
         3. JANGAN duplikat/parafrase fakta yang sudah ada di existing memory.\n\
         4. Tulis dari sudut pandang \"owner ...\" — satu kalimat ringkas per fakta.\n\
         5. Maksimal {MAX_FACTS_PER_REVIEW} fakta. Kalau tidak ada yang layak: []\n\
         6. Output HANYA JSON array valid, tanpa penjelasan, tanpa code fence:\n\
         [{{\"fact\": \"...\", \"type\": \"explicit\"|\"inferred\"}}]\n\
         \nExisting memory:\n{existing_block}"
    );
    let convo = format!("[owner]: {user_text}\n\n[assistant]: {reply}");
    let messages = vec![ApiMessage {
        role: "user".into(),
        content: vec![ContentBlock::Text { text: convo }],
    }];

    let text = call_llm_text(agent, &system, &messages).await?;
    let parsed = parse_json_array(&text)
        .context("review: output bukan JSON array")?;

    let mut saved = 0usize;
    let items = parsed.as_array().map(|a| a.iter()).unwrap_or_default();
    for item in items.take(MAX_FACTS_PER_REVIEW) {
        let fact = item["fact"].as_str().unwrap_or("").trim().to_string();
        let fact_type = match item["type"].as_str() {
            Some("inferred") => "inferred",
            _ => "explicit",
        };
        if fact.is_empty() || fact.chars().count() > MAX_FACT_CHARS {
            continue;
        }
        // anti-duplikat Rust-side (prompt sudah minta, ini safety net)
        if is_duplicate(&fact, &existing) {
            tracing::debug!(chat_id, fact = %fact, "review: fakta duplikat — skip");
            continue;
        }
        let out = memory::save_fact(&agent.pool, chat_id, &fact, fact_type).await?;
        tracing::info!(chat_id, fact = %fact, fact_type, "review: {out}");
        saved += 1;
    }
    if saved > 0 {
        tracing::info!(chat_id, saved, "background review selesai");
    }
    Ok(())
}

/// Duplikat kalau sama, atau satu kontain yang lain (setelah normalisasi).
fn is_duplicate(fact: &str, existing: &[memory::MemoryFact]) -> bool {
    let norm = |s: &str| s.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
    let n = norm(fact);
    existing.iter().any(|f| {
        let e = norm(&f.fact);
        e == n || e.contains(&n) || n.contains(&e)
    })
}

// ── Dreaming cycle (konsolidasi mingguan) ─────────────────────────────────────

/// Review seluruh memory (drop/rewrite/upgrade) + seluruh skills (delete/rewrite).
/// Konservatif: parse gagal → skip, aksi tidak valid → skip. Return ringkasan.
pub async fn run_dream(agent: &Agent) -> Result<String> {
    tracing::info!("dreaming cycle mulai");
    let mut summary = String::new();

    // A. Memory consolidation per chat
    let chat_ids = memory::distinct_chat_ids(&agent.pool).await?;
    let (mut drop_n, mut rewrite_n, mut upgrade_n) = (0, 0, 0);
    for chat_id in chat_ids {
        let facts = memory::list_facts(&agent.pool, chat_id, 500).await?;
        if facts.len() < 2 {
            continue; // kurang dari 2 → tidak ada yang bisa digabung; skip hemat token
        }
        let listing = facts
            .iter()
            .map(|f| format!("[{}] ({}) {}", f.id, f.fact_type, f.fact))
            .collect::<Vec<_>>()
            .join("\n");

        let system = format!(
            "Kamu modul konsolidasi memori asisten pribadi (\"dreaming\"). Review SELURUH \
             memory tersimpan di bawah ini dan usulkan aksi:\n\
             - \"drop\": tidak relevan lagi / basi / duplikat tumpang tindih\n\
             - \"rewrite\": gabungkan dua fakta jadi satu (rewrite satu + drop satunya) \
             atau perjelas redaksi — tulis teks barunya\n\
             - \"upgrade\": [inferred] yang sudah terkonfirmasi percakapan berikutnya → \
             jadi explicit\n\
             \nKONSERVATIF: kalau ragu, JANGAN usulkan apa pun. Fakta masih relevan = biarkan.\
             \nOutput HANYA JSON array (tanpa penjelasan/code fence), maksimal {MAX_DREAM_ACTIONS} aksi:\n\
             [{{\"action\":\"drop\",\"id\":3}},{{\"action\":\"rewrite\",\"id\":7,\"fact\":\"...\"}},\
             {{\"action\":\"upgrade\",\"id\":9}}]\n\
             Kalau tidak ada perubahan: []\n\
             \nMemory:\n{listing}"
        );
        let messages = vec![ApiMessage {
            role: "user".into(),
            content: vec![ContentBlock::Text { text: "Jalankan review.".into() }],
        }];

        let text = match call_llm_text(agent, &system, &messages).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(chat_id, "dream memory: LLM call gagal — skip: {e:#}");
                continue;
            }
        };
        let actions = match parse_json_array(&text) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(chat_id, "dream memory: parse gagal — skip: {e:#}");
                continue;
            }
        };
        let valid_ids: Vec<i64> = facts.iter().map(|f| f.id).collect();
        let actions_items = actions.as_array().map(|a| a.iter()).unwrap_or_default();
        for item in actions_items.take(MAX_DREAM_ACTIONS) {
            let Some(id) = item["id"].as_i64() else { continue };
            if !valid_ids.contains(&id) {
                continue; // id halusinasi — skip
            }
            let res: anyhow::Result<bool> = match item["action"].as_str().unwrap_or("") {
                "drop" => memory::delete_fact(&agent.pool, chat_id, id).await,
                "upgrade" => memory::set_fact_type(&agent.pool, chat_id, id, "explicit").await,
                "rewrite" => {
                    let fact = item["fact"].as_str().unwrap_or("").trim().to_string();
                    if fact.is_empty() || fact.chars().count() > MAX_FACT_CHARS {
                        continue;
                    }
                    memory::update_fact(&agent.pool, chat_id, id, &fact).await
                }
                _ => continue,
            };
            match res {
                Ok(true) => match item["action"].as_str().unwrap_or("") {
                    "drop" => drop_n += 1,
                    "upgrade" => upgrade_n += 1,
                    _ => rewrite_n += 1,
                },
                Ok(false) => {}
                Err(e) => tracing::warn!(chat_id, id, "dream memory: aksi gagal: {e:#}"),
            }
        }
    }
    summary.push_str(&format!(
        "🧠 memory: {drop_n} dihapus, {rewrite_n} digabung/ditulis ulang, {upgrade_n} inferred→explicit."
    ));

    // B. Skills review
    let dir: std::path::PathBuf = agent.cfg.skills_dir.clone().into();
    let metas = skills::list_skills(&dir);
    let (mut sdel, mut srew) = (0, 0);
    if !metas.is_empty() {
        let mut listing = String::new();
        for m in &metas {
            let content = std::fs::read_to_string(dir.join(&m.filename))
                .unwrap_or_default()
                .chars()
                .take(4000)
                .collect::<String>();
            listing.push_str(&format!("\n### {}\n{}\n", m.filename, content.trim_end()));
        }
        let system = format!(
            "Kamu modul review library skill asisten pribadi (\"dreaming\"). Skill = file \
             markdown berisi prosedur (langkah, command, gotchas). Review SEMUA skill:\n\
             - \"delete\": salah / benar-benar kedaluwarsa / duplikat (untuk MERGE dua skill: \
             rewrite yang satu dengan isi gabungan, delete satunya)\n\
             - \"rewrite\": update isi (langkah baru, gotcha tambahan, revisi command) — tulis \
             KONTEN PENUH hasil revisi\n\
             \nKONSERVATIF: kalau ragu, JANGAN usulkan apa pun.\n\
             Output HANYA JSON array (tanpa penjelasan/code fence):\n\
             [{{\"action\":\"delete\",\"file\":\"x.md\"}},{{\"action\":\"rewrite\",\"file\":\"y.md\",\
             \"content\":\"...\"}}]\n\
             Kalau tidak ada perubahan: []\n{listing}"
        );
        let messages = vec![ApiMessage {
            role: "user".into(),
            content: vec![ContentBlock::Text { text: "Jalankan review.".into() }],
        }];
        if let Ok(text) = call_llm_text(agent, &system, &messages).await {
            if let Ok(actions) = parse_json_array(&text) {
                let valid: Vec<&str> = metas.iter().map(|m| m.filename.as_str()).collect();
                let skill_items = actions.as_array().map(|a| a.iter()).unwrap_or_default();
                for item in skill_items.take(MAX_DREAM_ACTIONS) {
                    let Some(file) = item["file"].as_str() else { continue };
                    if !valid.contains(&file) {
                        continue;
                    }
                    match item["action"].as_str().unwrap_or("") {
                        "delete" => match skills::delete_skill(&dir, file) {
                            Ok(true) => sdel += 1,
                            Ok(false) => {}
                            Err(e) => tracing::warn!("dream skills: hapus gagal: {e:#}"),
                        },
                        "rewrite" => {
                            let content = item["content"].as_str().unwrap_or("").trim().to_string();
                            if content.chars().count() < 80 {
                                continue;
                            }
                            match skills::rewrite_skill(&dir, file, &content) {
                                Ok(()) => srew += 1,
                                Err(e) => tracing::warn!("dream skills: rewrite gagal: {e:#}"),
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        summary.push_str(&format!("\n📚 skills: {} file direview, {sdel} dihapus, {srew} di-rewrite.", metas.len()));
    }

    tracing::info!(summary = %summary, "dreaming cycle selesai");
    Ok(summary)
}

// ── Helpers ────────────────────────────────────────────────────────────────────

/// LLM call internal (tanpa tools), return gabungan text blocks + catat usage.
async fn call_llm_text(agent: &Agent, system: &str, messages: &[ApiMessage]) -> Result<String> {
    let resp = agent.provider.chat(system, messages, &[]).await?;
    {
        let mut u = agent.usage.lock().unwrap();
        u.input_tokens += resp.usage.input_tokens;
        u.output_tokens += resp.usage.output_tokens;
        u.turns += 1;
    }
    let text = resp
        .blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    if text.trim().is_empty() {
        bail!("respons LLM kosong");
    }
    Ok(text)
}

/// Parse JSON array dari output LLM yang bandel: buang code fence, ambil dari
/// '[' pertama sampai ']' terakhir.
fn parse_json_array(text: &str) -> Result<Value> {
    let cleaned = text.trim().trim_start_matches("```json").trim_start_matches("```");
    let cleaned = cleaned.trim_end_matches("```").trim();
    let Some(start) = cleaned.find('[') else {
        bail!("tidak ada '[' dalam output");
    };
    let Some(end) = cleaned.rfind(']') else {
        bail!("tidak ada ']' dalam output");
    };
    let slice = &cleaned[start..=end];
    serde_json::from_str(slice).with_context(|| format!("JSON tidak valid: {}", &slice[..slice.len().min(200)]))
}

// Dipakai unit test utk verifikasi bentuk aksi dreaming tanpa network.
#[cfg(test)]
#[derive(serde::Deserialize, Debug)]
struct _DreamActionShape {
    action: String,
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    fact: Option<String>,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    content: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_json_array_robust() {
        let cases = [
            r#"[{"fact":"a","type":"explicit"}]"#,
            "```json\n[{\"fact\":\"a\",\"type\":\"inferred\"}]\n```",
            "Baik, hasilnya:\n[{\"fact\":\"a\"}] terima kasih",
            "[]",
        ];
        for c in cases {
            assert!(parse_json_array(c).is_ok(), "gagal parse: {c}");
        }
        assert!(parse_json_array("tidak ada array").is_err());
        assert!(parse_json_array("[broken").is_err());
    }

    #[test]
    fn duplicate_detection() {
        let existing = vec![memory::MemoryFact {
            id: 1,
            fact: "Owner suka deploy di malam hari".into(),
            fact_type: "explicit".into(),
        }];
        assert!(is_duplicate("owner suka deploy di malam hari", &existing)); // persis
        assert!(is_duplicate("Owner suka   deploy di malam hari sekali", &existing)); // containment
        assert!(is_duplicate("owner suka deploy", &existing)); // subset
        assert!(!is_duplicate("owner pakai VPS di Jakarta", &existing));
    }

    #[test]
    fn dream_action_shape_parses() {
        let v = parse_json_array(
            r#"[{"action":"drop","id":3},{"action":"rewrite","id":7,"fact":"baru"},{"action":"upgrade","id":9}]"#,
        )
        .unwrap();
        let acts: Vec<_DreamActionShape> = serde_json::from_value(v).unwrap();
        assert_eq!(acts.len(), 3);
        assert_eq!(acts[0].action, "drop");
        assert_eq!(acts[1].id, Some(7));
        assert_eq!(acts[2].action, "upgrade");
    }
}
