//! Self-improvement loop (Pilar 6, ROADMAP Fase 6) — BUKAN fine-tuning.
//! Model tetap statis; yang "improve" adalah ekstraksi & kurasi memory.
//!
//! Dua proses, keduanya via LLM call internal tanpa tools (JSON output):
//! 1. **Background review pasca-turn** — `tokio::spawn` fire-and-forget setelah
//!    tiap turn owner (via gateway): analisis pertukaran terakhir, ekstrak fakta
//!    `[explicit]` / `[inferred]`, anti-duplikat dengan memory existing.
//! 2. **Dreaming cycle** — berkala (mingguan, plus `/dream` manual): review
//!    SELURUH memory (drop/patch/rewrite/upgrade) dan SELURUH skills (delete/rewrite/merge).
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
/// Batas SEARCH/REPLACE blocks per aksi patch (fail-safe).
const MAX_PATCH_BLOCKS_PER_ACTION: usize = 5;
/// Ambang kemiripan fuzzy fallback pencocokan SEARCH (levenshtein ratio).
const FUZZY_MIN_RATIO: f64 = 0.8;

// ── Post-turn background review ────────────────────────────────────────────────

/// Spawn review fire-and-forget — TIDAK menambah latency respons owner (Pilar 6).
pub fn spawn_post_turn_review(
    agent: std::sync::Arc<Agent>,
    chat_id: i64,
    user_text: String,
    reply: String,
) {
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
    let parsed = parse_json_array(&text).context("review: output bukan JSON array")?;

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
    let norm = |s: &str| {
        s.to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let n = norm(fact);
    existing.iter().any(|f| {
        let e = norm(&f.fact);
        e == n || e.contains(&n) || n.contains(&e)
    })
}

// ── Patch merge op (adopsi SearchReplaceBlock OpenViking) ──────────────────────

/// Satu block patch: ganti kemunculan `search` → `replace` dalam teks fakta.
struct PatchBlock {
    search: String,
    replace: String,
}

/// Normalisasi unicode ringan untuk pencocokan: smart quotes → straight,
/// nbsp-variants → spasi, zero-width chars dibuang. 1 char asli → ≤1 char
/// hasil, sehingga posisi match tetap terpetakan balik ke string asli.
fn normalize_for_match(s: &str) -> String {
    s.chars()
        .filter_map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{201B}' => Some('\''),
            '\u{201C}' | '\u{201D}' => Some('"'),
            '\u{00A0}' | '\u{2007}' | '\u{202F}' => Some(' '),
            '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' => None,
            _ => Some(c),
        })
        .collect()
}

/// Levenshtein distance (char-based) — fakta ≤300 char, O(n²) murah.
fn levenshtein(a: &[char], b: &[char]) -> usize {
    let (m, n) = (a.len(), b.len());
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut cur: Vec<usize> = vec![0; n + 1];
    for i in 1..=m {
        cur[0] = i;
        for j in 1..=n {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[n]
}

/// Terapkan SEARCH/REPLACE blocks ke satu fakta (chained: block ke-n di-apply
/// ke hasil block n-1). Fail-closed: satu block gagal (tidak ketemu / ambigu /
/// hasil kosong / kepanjangan) → seluruh patch DITOLAK, fakta tidak disentuh.
fn apply_patch(fact: &str, blocks: &[PatchBlock]) -> Result<String> {
    let mut current = fact.trim().to_string();
    for (i, b) in blocks.iter().enumerate() {
        current = apply_block(&current, b).with_context(|| format!("block #{} gagal", i + 1))?;
    }
    if current.trim().is_empty() {
        bail!("hasil patch kosong — untuk menghapus fakta pakai action \"drop\"");
    }
    if current.chars().count() > MAX_FACT_CHARS {
        bail!(
            "hasil patch {} karakter (cap {MAX_FACT_CHARS})",
            current.chars().count()
        );
    }
    Ok(current)
}

/// Terapkan satu block: exact match dulu (wajib unik), fallback fuzzy
/// (levenshtein ratio ≥ FUZZY_MIN_RATIO, kandidat terbaik wajib unik).
/// Pencocokan di domain normalized-unicode; replacement di-splice pada
/// string ASLI sehingga karakter di luar area match tidak tersentuh.
fn apply_block(hay: &str, block: &PatchBlock) -> Result<String> {
    // Parallel per char: (byte_start, byte_end) di string ASLI + char normalized.
    let mut nchars: Vec<(usize, usize, char)> = Vec::new();
    for (s, c) in hay.char_indices() {
        let e = s + c.len_utf8();
        let nc = match c {
            '\u{2018}' | '\u{2019}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            '\u{00A0}' | '\u{2007}' | '\u{202F}' => ' ',
            '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' => continue,
            _ => c,
        };
        nchars.push((s, e, nc));
    }
    let ntext: Vec<char> = nchars.iter().map(|(_, _, c)| *c).collect();
    let pat: Vec<char> = normalize_for_match(&block.search).chars().collect();
    let m = pat.len();
    if m == 0 {
        bail!("SEARCH kosong (atau hanya whitespace/zero-width)");
    }
    if ntext.len() < m {
        bail!(
            "SEARCH ({m} char) lebih panjang dari fakta ({} char)",
            ntext.len()
        );
    }

    // 1) Exact match — kumpulkan semua kemunculan.
    let mut exact: Vec<usize> = Vec::new();
    for i in 0..=ntext.len() - m {
        if ntext[i..i + m] == pat[..] {
            exact.push(i);
        }
    }
    let hit = match exact.len() {
        1 => exact[0],
        0 => {
            // 2) Fuzzy fallback — sliding window, kandidat terbaik harus unik.
            let mut best: Option<(f64, usize)> = None;
            let mut tie = false;
            for i in 0..=ntext.len() - m {
                let dist = levenshtein(&ntext[i..i + m], &pat);
                let ratio = 1.0 - dist as f64 / m.max(pat.len()) as f64;
                if ratio < FUZZY_MIN_RATIO {
                    continue;
                }
                match best {
                    Some((r, _)) if (ratio - r).abs() < f64::EPSILON * 4.0 => tie = true,
                    Some((r, _)) if ratio > r => {
                        best = Some((ratio, i));
                        tie = false;
                    }
                    Some(_) => {}
                    None => best = Some((ratio, i)),
                }
            }
            match best {
                Some((r, i)) if !tie => {
                    tracing::debug!(ratio = r, "patch: fuzzy match");
                    i
                }
                Some(_) => bail!(
                    "SEARCH ambigu: beberapa kandidat fuzzy sama kuat — perpanjang/persiskan SEARCH"
                ),
                None => bail!(
                    "SEARCH tidak ditemukan (exact maupun fuzzy ≥{FUZZY_MIN_RATIO}): {:?}",
                    block.search
                ),
            }
        }
        n => bail!("SEARCH ambigu: {n} kemunculan exact — perpanjang SEARCH agar unik"),
    };

    // 3) Splice replacement pada string ASLI (bukan hasil normalisasi).
    let (bs, _, _) = nchars[hit];
    let (_, be, _) = nchars[hit + m - 1];
    let mut out = String::with_capacity(hay.len() + block.replace.len());
    out.push_str(&hay[..bs]);
    out.push_str(&block.replace);
    out.push_str(&hay[be..]);
    Ok(out)
}

// ── Dreaming cycle (konsolidasi mingguan) ─────────────────────────────────────

/// Review seluruh memory (drop/patch/rewrite/upgrade) + seluruh skills (delete/rewrite).
/// Konservatif: parse gagal → skip, aksi tidak valid → skip. Return ringkasan.
pub async fn run_dream(agent: &Agent) -> Result<String> {
    tracing::info!("dreaming cycle mulai");
    let mut summary = String::new();

    // A. Memory consolidation per chat
    let chat_ids = memory::distinct_chat_ids(&agent.pool).await?;
    let (mut drop_n, mut patch_n, mut rewrite_n, mut upgrade_n) = (0, 0, 0, 0);
    for chat_id in chat_ids {
        let facts = memory::list_facts(&agent.pool, chat_id, 500).await?;
        if facts.len() < 2 {
            continue; // kurang dari 2 → tidak ada yang bisa digabung; skip hemat token
        }
        // Hotness per fakta (adopsi OpenViking): skor 0-1, cold = jarang diakses & tua.
        // LLM dipandu drop yang cold duluan, bukan asal umur entri.
        let now = chrono::Utc::now();
        let listing = facts
            .iter()
            .map(|f| {
                let h = memory::hotness(f.access_count, f.accessed_at, now);
                format!("[{}] ({}) hotness={:.2} {}", f.id, f.fact_type, h, f.fact)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let system = format!(
            "Kamu modul konsolidasi memori asisten pribadi (\"dreaming\"). Review SELURUH \
             memory tersimpan di bawah ini dan usulkan aksi:\n\
             - \"drop\": tidak relevan lagi / basi / duplikat tumpang tindih. Prioritaskan hotness rendah (<0.10 = jarang diakses & lama tak dipakai)\n\
             - \"patch\": edit presisi SATU fakta (update angka/tanggal/status, hapus bagian basi) \
             tanpa menulis ulang sisanya. blocks = daftar {{\"search\",\"replace\"}}: \
             search = substring LAMA disalin PERSIS dari fakta (min. 10 karakter, harus unik \
             dalam fakta itu), replace = teks pengganti (\"\" = hapus bagian itu). Patch yang \
             search-nya tidak ditemukan/ambigu akan DITOLAK otomatis — salin akurat.\n\
             - \"rewrite\": gabungkan dua fakta jadi satu (rewrite satu + drop satunya) \
             atau restrukturisasi total — tulis teks barunya penuh\n\
             - \"upgrade\": [inferred] yang sudah terkonfirmasi percakapan berikutnya → \
             jadi explicit\n\
             \nPREFER \"patch\" untuk edit kecil; \"rewrite\" hanya untuk merge dua fakta atau \
             restrukturisasi total.\n\
             \nKONSERVATIF: kalau ragu, JANGAN usulkan apa pun. Fakta masih relevan = biarkan.\
             \nOutput HANYA JSON array (tanpa penjelasan/code fence), maksimal {MAX_DREAM_ACTIONS} aksi:\n\
             [{{\"action\":\"drop\",\"id\":3}},\
             {{\"action\":\"patch\",\"id\":7,\"blocks\":[{{\"search\":\"...\",\"replace\":\"...\"}}]}},\
             {{\"action\":\"rewrite\",\"id\":8,\"fact\":\"...\"}},\
             {{\"action\":\"upgrade\",\"id\":9}}]\n\
             Kalau tidak ada perubahan: []\n\
             \nMemory:\n{listing}"
        );
        let messages = vec![ApiMessage {
            role: "user".into(),
            content: vec![ContentBlock::Text {
                text: "Jalankan review.".into(),
            }],
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
            let Some(id) = item["id"].as_i64() else {
                continue;
            };
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
                "patch" => {
                    let Some(blocks_json) = item["blocks"].as_array() else {
                        tracing::warn!(chat_id, id, "dream patch: tanpa blocks — skip");
                        continue;
                    };
                    let Some(f) = facts.iter().find(|f| f.id == id) else {
                        continue;
                    };
                    let fact_txt = f.fact.clone();
                    let mut blocks: Vec<PatchBlock> = Vec::new();
                    let mut valid = true;
                    for b in blocks_json.iter().take(MAX_PATCH_BLOCKS_PER_ACTION) {
                        match (b["search"].as_str(), b["replace"].as_str()) {
                            (Some(s), Some(r)) if !s.trim().is_empty() => {
                                blocks.push(PatchBlock {
                                    search: s.to_string(),
                                    replace: r.to_string(),
                                });
                            }
                            _ => {
                                valid = false;
                                break;
                            }
                        }
                    }
                    if !valid || blocks.is_empty() {
                        tracing::warn!(
                            chat_id,
                            id,
                            "dream patch: blocks invalid/kosong — skip (fail-closed)"
                        );
                        continue;
                    }
                    match apply_patch(&fact_txt, &blocks) {
                        Ok(new_fact) => {
                            memory::update_fact(&agent.pool, chat_id, id, &new_fact).await
                        }
                        Err(e) => {
                            tracing::warn!(chat_id, id, "dream patch ditolak: {e:#}");
                            Ok(false)
                        }
                    }
                }
                _ => continue,
            };
            match res {
                Ok(true) => match item["action"].as_str().unwrap_or("") {
                    "drop" => drop_n += 1,
                    "upgrade" => upgrade_n += 1,
                    "patch" => patch_n += 1,
                    _ => rewrite_n += 1,
                },
                Ok(false) => {}
                Err(e) => tracing::warn!(chat_id, id, "dream memory: aksi gagal: {e:#}"),
            }
        }
    }
    summary.push_str(&format!(
        "🧠 memory: {drop_n} dihapus, {patch_n} di-patch, {rewrite_n} digabung/ditulis ulang, {upgrade_n} inferred→explicit."
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
             KONTEN PENUH hasil revisi. File skill berawalan blok frontmatter \
             `---\ndescription: ...\n---` (L0 utk matching topik): PERTAHANKAN blok itu di kepala \
             hasil rewrite, perbarui `description:` kalau cakupan skill berubah (satu kalimat, \
             ≤160 char). Konten rewrite TANPA frontmatter akan otomatis di-prepend description \
             lama oleh sistem.\n\
             \nKONSERVATIF: kalau ragu, JANGAN usulkan apa pun.\n\
             Output HANYA JSON array (tanpa penjelasan/code fence):\n\
             [{{\"action\":\"delete\",\"file\":\"x.md\"}},{{\"action\":\"rewrite\",\"file\":\"y.md\",\
             \"content\":\"...\"}}]\n\
             Kalau tidak ada perubahan: []\n{listing}"
        );
        let messages = vec![ApiMessage {
            role: "user".into(),
            content: vec![ContentBlock::Text {
                text: "Jalankan review.".into(),
            }],
        }];
        if let Ok(text) = call_llm_text(agent, &system, &messages).await {
            if let Ok(actions) = parse_json_array(&text) {
                let valid: Vec<&str> = metas.iter().map(|m| m.filename.as_str()).collect();
                let skill_items = actions.as_array().map(|a| a.iter()).unwrap_or_default();
                for item in skill_items.take(MAX_DREAM_ACTIONS) {
                    let Some(file) = item["file"].as_str() else {
                        continue;
                    };
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
        summary.push_str(&format!(
            "\n📚 skills: {} file direview, {sdel} dihapus, {srew} di-rewrite.",
            metas.len()
        ));
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
    let cleaned = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```");
    let cleaned = cleaned.trim_end_matches("```").trim();
    let Some(start) = cleaned.find('[') else {
        bail!("tidak ada '[' dalam output");
    };
    let Some(end) = cleaned.rfind(']') else {
        bail!("tidak ada ']' dalam output");
    };
    let slice = &cleaned[start..=end];
    serde_json::from_str(slice)
        .with_context(|| format!("JSON tidak valid: {}", &slice[..slice.len().min(200)]))
}

// Dipakai unit test utk verifikasi bentuk aksi dreaming tanpa network.
#[cfg(test)]
#[derive(serde::Deserialize, Debug)]
struct _DreamBlockShape {
    search: String,
    replace: String,
}

#[cfg(test)]
#[derive(serde::Deserialize, Debug)]
struct _DreamActionShape {
    action: String,
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    fact: Option<String>,
    #[serde(default)]
    blocks: Option<Vec<_DreamBlockShape>>,
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
            access_count: 0,
            accessed_at: chrono::Utc::now(),
        }];
        assert!(is_duplicate("owner suka deploy di malam hari", &existing)); // persis
        assert!(is_duplicate(
            "Owner suka   deploy di malam hari sekali",
            &existing
        )); // containment
        assert!(is_duplicate("owner suka deploy", &existing)); // subset
        assert!(!is_duplicate("owner pakai VPS di Jakarta", &existing));
    }

    #[test]
    fn dream_action_shape_parses() {
        let v = parse_json_array(
            r#"[{"action":"drop","id":3},{"action":"rewrite","id":7,"fact":"baru"},{"action":"upgrade","id":9},{"action":"patch","id":12,"blocks":[{"search":"lama","replace":"baru"}]}]"#,
        )
        .unwrap();
        let acts: Vec<_DreamActionShape> = serde_json::from_value(v).unwrap();
        assert_eq!(acts.len(), 4);
        assert_eq!(acts[0].action, "drop");
        assert_eq!(acts[1].id, Some(7));
        assert_eq!(acts[2].action, "upgrade");
        assert_eq!(acts[3].action, "patch");
        assert_eq!(acts[3].blocks.as_ref().unwrap()[0].search, "lama");
    }

    // ── Patch merge op ────────────────────────────────────────────────────────

    fn pb(search: &str, replace: &str) -> PatchBlock {
        PatchBlock {
            search: search.into(),
            replace: replace.into(),
        }
    }

    #[test]
    fn patch_exact_single_match() {
        let out = apply_patch(
            "Owner target pace 6:00/km dalam 6-12 bulan",
            &[pb("6:00/km", "5'30\"/km")],
        )
        .unwrap();
        assert_eq!(out, "Owner target pace 5'30\"/km dalam 6-12 bulan");
    }

    #[test]
    fn patch_multiple_blocks_chained() {
        let out = apply_patch(
            "Race 10K 1 Jan, pace 6:45/km",
            &[pb("6KM", "10K"), pb("6:45/km", "6:50/km")],
        )
        .unwrap();
        assert_eq!(out, "Race 10K 23 Agu, pace 6:50/km");
    }

    #[test]
    fn patch_ambiguous_exact_rejected() {
        // SEARCH muncul 2x → ambigu → ditolak (fail-closed).
        let err = apply_patch(
            "Gaji Rp1.234.567 dan bonus Rp1.234.567 tahunan",
            &[pb("Rp1.234.567", "X")],
        );
        assert!(err.is_err());
        assert!(format!("{:#}", err.unwrap_err()).contains("ambigu"));
    }

    #[test]
    fn patch_not_found_rejected() {
        let err = apply_patch("Owner lari 3x seminggu", &[pb("gym membership", "y")]);
        assert!(err.is_err());
        assert!(format!("{:#}", err.unwrap_err()).contains("tidak ditemukan"));
    }

    #[test]
    fn patch_empty_search_rejected() {
        let err = apply_patch("Owner lari 3x seminggu", &[pb("   ", "x")]);
        assert!(err.is_err());
    }

    #[test]
    fn patch_to_empty_result_rejected() {
        // Hasil kosong = itu drop, bukan patch — ditolak.
        let err = apply_patch("abcd efgh ijkl", &[pb("abcd efgh ijkl", "")]);
        assert!(err.is_err());
        assert!(format!("{:#}", err.unwrap_err()).contains("drop"));
    }

    #[test]
    fn patch_fuzzy_tolerates_typos() {
        // SEARCH beda 1 char (':' vs ';') → ratio ~0.95, fuzzy fallback jalan.
        let out = apply_patch(
            "Watch HR alert ceiling Z2: 133 bpm",
            &[pb("ceiling Z2; 133 bpm", "ceiling Z2: 140 bpm")],
        )
        .unwrap();
        assert_eq!(out, "Watch HR alert ceiling Z2: 140 bpm");
    }

    #[test]
    fn patch_fuzzy_rejects_far_mismatch() {
        // Beda jauh → di bawah threshold → ditolak.
        let err = apply_patch(
            "Watch HR alert ceiling Z2: 133 bpm",
            &[pb("ceiling xxxxxxxxxxxxxxx", "y")],
        );
        assert!(err.is_err());
    }

    #[test]
    fn patch_unicode_normalization() {
        // Fakta asli pakai smart quotes; SEARCH pakai straight quotes → tetap ketemu.
        let fact = "Owner bilang \u{201C}deploy malam\u{201D} itu enak";
        let out = apply_patch(fact, &[pb("\"deploy malam\"", "'deploy malam'")]).unwrap();
        assert_eq!(out, "Owner bilang 'deploy malam' itu enak");
    }

    #[test]
    fn patch_atomic_per_fact() {
        // Block #2 gagal → seluruh patch ditolak, block #1 tidak diterapkan.
        let err = apply_patch(
            "Race 10K 1 Jan, pace 6:45/km",
            &[pb("6KM", "10K"), pb("tidak ada string ini", "x")],
        );
        assert!(err.is_err());
    }

    #[test]
    fn patch_result_over_cap_rejected() {
        let long = "x".repeat(295);
        let err = apply_patch(&format!("{long} tail"), &[pb("tail", &"y".repeat(50))]);
        assert!(err.is_err());
    }
}
