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
use crate::provider::{ApiMessage, CallOpts, ContentBlock};
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
/// Batas aksi saat budget explicit penuh (≥BUDGET_PRESSURE_PCT) — memangkas
/// 45+ fakta ke target 80% cap butuh >20 aksi (tiap merge = rewrite + drop
/// berpasangan). Tanpa ini cycle tekanan mentok di 20 aksi dan butuh banyak
/// /dream berulang untuk satu kali konsolidasi.
const MAX_DREAM_ACTIONS_UNDER_PRESSURE: usize = 40;
/// Iterasi LLM per chat dalam satu /dream — pass lanjutan hanya jalan selama
/// masih over budget (lihat run_dream). Model sering gagal capai target char
/// dalam satu pass (empiris 5 Sep: -316 dari kebutuhan -1773 karena memilih
/// patch kecil) — iterasi dengan feedback sisa kebutuhan lebih andal daripada
/// prompt yang lebih keras.
const MAX_DREAM_PASSES: usize = 3;
/// Kemajuan minimum per pass (char explicit) — di bawah ini pass berikutnya
/// dianggap sia-sia (kandidat habis) dan iterasi dihentikan.
const MIN_PROGRESS_CHARS: i64 = 50;
/// Batas output token khusus dream — JSON aksi konsolidasi (hingga 20 aksi +
/// patch blocks) butuh ruang; 8192 default sering habis oleh thinking model.
const DREAM_MAX_TOKENS: u32 = 16_384;
/// Ambang tekanan budget explicit (% dari cap memory) — di atas ini dreaming
/// diarahkan memangkas char & upgrade inferred→explicit ditolak (menambah char).
const BUDGET_PRESSURE_PCT: i64 = 85;
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
            .map(|f| format!("- {} [{}|{}]", f.fact, f.fact_type, f.kind))
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
         3. Kategori (kind) fakta:\n\
         - \"profile\": identitas dasar owner — nama, lokasi, pekerjaan, keluarga\n\
         - \"preference\": preferensi & kebiasaan — \"deploy suka jam malam\", \"jawaban singkat\"\n\
         - \"entity\": orang/proyek/objek yang berulang — VPS, domain, stack, proyek aktif\n\
         - \"event\": peristiwa & keputusan bertanggal — \"23 Agu ikut race 10K\"\n\
         - \"general\": sisanya — kalau ragu, pakai general\n\
         4. JANGAN duplikat/parafrase fakta yang sudah ada di existing memory.\n\
         5. Tulis dari sudut pandang \"owner ...\" — satu kalimat ringkas per fakta.\n\
         6. Maksimal {MAX_FACTS_PER_REVIEW} fakta. Kalau tidak ada yang layak: []\n\
         7. Output HANYA JSON array valid, tanpa penjelasan, tanpa code fence:\n\
         [{{\"fact\": \"...\", \"type\": \"explicit\"|\"inferred\", \
         \"kind\": \"profile\"|\"preference\"|\"entity\"|\"event\"|\"general\"}}]\n\
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
        let kind = item["kind"].as_str().unwrap_or("");
        let kind = if memory::valid_kind(kind) { kind } else { "general" };
        if fact.is_empty() || fact.chars().count() > MAX_FACT_CHARS {
            continue;
        }
        // anti-duplikat Rust-side (prompt sudah minta, ini safety net)
        if is_duplicate(&fact, &existing) {
            tracing::debug!(chat_id, fact = %fact, "review: fakta duplikat — skip");
            continue;
        }
        let out =
            memory::save_fact(&agent.pool, chat_id, &fact, fact_type, kind, "review").await?;
        tracing::info!(chat_id, fact = %fact, fact_type, kind, "review: {out}");
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

/// Paragraf tekanan budget utk prompt dreaming — None kalau explicit masih lega
/// (< BUDGET_PRESSURE_PCT dari cap). Over threshold = cycle ini WAJIB memangkas
/// total char explicit (drop/merge) — menimpa aturan konservatif, karena tanpa
/// ini dreaming selalu konservatif dan cap 9k tidak pernah turun (insiden 5 Sep:
/// warning "cap tercapai" persisten meski /dream diulang).
///
/// Cycle pertama pasca-fix (5 Sep 12:05) membuktikan paragraf generik tidak
/// cukup: LLM mendamaikannya dengan petunjuk "drop = hotness rendah" lalu justru
/// drop 21 baris INFERRED (semuanya cold) — explicit 8973→8973, cycle gagal.
/// Maka paragraf ini kini eksplisit: inferred bukan target (tidak membebani
/// cap), hotness explicit = artefak tracking lama (abaikan), dan ada angka char
/// wajib dipangkas supaya sukses cycle terukur.
fn budget_pressure(explicit_chars: i64) -> Option<String> {
    let cap = memory::MAX_EXPLICIT_CHARS;
    if explicit_chars * 100 < cap * BUDGET_PRESSURE_PCT {
        return None;
    }
    let target = cap * 80 / 100;
    Some(format!(
        "\n⚠️ BUDGET EXPLICIT PENUH: {chars}/{cap} karakter (cap = hot tier system \
         prompt; penuh = fakta explicit baru DITOLAK). PRIORITAS SATU-SATUNYA cycle ini \
         (MENIMPA aturan konservatif dan petunjuk hotness di bawah): turunkan total char \
         baris (explicit) minimal {need} char, sampai target ≤{target}.\n\
         - HANYA usulkan aksi atas baris (explicit): drop yang basi / kedaluwarsa / \
         tumpang tindih, dan MERGE fakta serumpun jadi satu baris padat (rewrite satu + \
         drop pasangannya).\
         - Minimal pangkas {need} char ≈ {rows} baris penuh (±200 char/baris). Banyak \
         patch kecil TIDAK akan mencukupi — UTAMAKAN drop & merge, bukan patch.\n\
         - JANGAN drop/patch/reclassify baris (inferred) cycle ini — inferred TIDAK \
         membebani cap ini; menghabiskan aksi ke sana = cycle gagal.\n\
         - JANGAN upgrade inferred→explicit (menambah char).\n\
         - Hotness baris explicit bisa tinggi karena artefak tracking lama — ABAIKAN \
         hotness; pilih kandidat drop dari ISI fakta, bukan dari angka hotness.",
        chars = explicit_chars,
        need = explicit_chars - target,
        rows = (explicit_chars - target + 199) / 200,
        target = target,
    ))
}

/// Review seluruh memory (drop/patch/rewrite/upgrade) + seluruh skills (delete/rewrite).
/// Konservatif: parse gagal → skip, aksi tidak valid → skip. Return ringkasan.
pub async fn run_dream(agent: &Agent) -> Result<String> {
    tracing::info!("dreaming cycle mulai");
    let mut summary = String::new();

    // A. Memory consolidation per chat
    let chat_ids = memory::distinct_chat_ids(&agent.pool).await?;
    let (mut drop_n, mut patch_n, mut rewrite_n, mut upgrade_n, mut reclass_n) =
        (0, 0, 0, 0, 0);
    let (mut explicit_before_total, mut explicit_after_total) = (0i64, 0i64);
    let mut pass_total = 0usize;
    for chat_id in chat_ids {
        // Baseline laporan before→after — diukur SEBELUM pass pertama.
        explicit_before_total += memory::explicit_chars(&agent.pool, chat_id).await?;
        // Multi-pass: pass 0 = konsolidasi normal (reclassify dll); pass lanjutan
        // HANYA kalau masih over budget — tiap pass melihat sisa kebutuhan char
        // terbaru. Satu pass saja terbukti tidak memenuhi target kuantitatif
        // (empiris 5 Sep: -316 char dari kebutuhan -1773 — model pilih patch kecil).
        for pass in 0..MAX_DREAM_PASSES {
            let facts = memory::list_facts(&agent.pool, chat_id, 500).await?;
            if facts.len() < 2 {
                break; // kurang dari 2 → tidak ada yang bisa digabung; hemat token
            }
            let explicit_before = memory::explicit_chars(&agent.pool, chat_id).await?;
            let budget = budget_pressure(explicit_before).unwrap_or_default();
            let over_budget = !budget.is_empty();
            if pass > 0 && !over_budget {
                break; // target tercapai — cukup
            }
            let max_actions = if over_budget {
                MAX_DREAM_ACTIONS_UNDER_PRESSURE
            } else {
                MAX_DREAM_ACTIONS
            };
            let tally =
                match dream_pass(agent, chat_id, &facts, &budget, max_actions, over_budget).await
                {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::warn!(chat_id, pass, "dream memory pass gagal — skip: {e:#}");
                        break;
                    }
                };
            drop_n += tally.drops;
            patch_n += tally.patches;
            rewrite_n += tally.rewrites;
            upgrade_n += tally.upgrades;
            reclass_n += tally.reclassifies;
            pass_total += 1;
            // Kemajuan nihil saat over budget → pass berikutnya kemungkinan besar
            // sama (kandidat habis) — stop, jangan bakar token percuma.
            let explicit_after = memory::explicit_chars(&agent.pool, chat_id).await?;
            if over_budget && explicit_before - explicit_after < MIN_PROGRESS_CHARS {
                tracing::warn!(
                    chat_id,
                    pass,
                    "dream pass tanpa kemajuan (<{MIN_PROGRESS_CHARS} char) — stop iterasi"
                );
                break;
            }
        }
        explicit_after_total += memory::explicit_chars(&agent.pool, chat_id).await?;
    }

    // Helper lokal (item dalam blok) — isi pass konsolidasi; lihat doc di bawah.
    /// Hitungan aksi satu pass konsolidasi (diagregasi lintas pass di run_dream).
    #[derive(Default)]
    struct DreamTally {
        drops: usize,
        patches: usize,
        rewrites: usize,
        upgrades: usize,
        reclassifies: usize,
    }

    /// Satu pass konsolidasi memory utk satu chat: listing → prompt (dengan
    /// paragraf tekanan budget bila over) → LLM → apply aksi. Orkestrasi
    /// multi-pass ada di run_dream; fn ini tidak tahu iterasi. Fail-soft per
    /// aksi tetap berlaku (aksi invalid di-skip, bukan membatalkan pass).
    async fn dream_pass(
        agent: &Agent,
        chat_id: i64,
        facts: &[memory::MemoryFact],
        budget: &str,
        max_actions: usize,
        over_budget: bool,
    ) -> Result<DreamTally> {
    let mut t = DreamTally::default();
        // Hotness per fakta (adopsi OpenViking): skor 0-1, cold = jarang diakses & tua.
        // LLM dipandu drop yang cold duluan, bukan asal umur entri.
        let now = chrono::Utc::now();
        let listing = facts
            .iter()
            .map(|f| {
                let h = memory::hotness(f.access_count, f.accessed_at, now);
                format!(
                    "[{}] ({}|{}) hotness={:.2} {}",
                    f.id, f.fact_type, f.kind, h, f.fact
                )
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
             - \"reclassify\": ubah kategori (kind) fakta. Taxonomi: profile = identitas \
             dasar owner; preference = preferensi & kebiasaan; entity = orang/proyek/objek \
             berulang; event = peristiwa & keputusan bertanggal; general = sisanya. \
             Prioritaskan baris yang masih kind=general (backfill baris lama).\n\
             \nPREFER \"patch\" untuk edit kecil; \"rewrite\" hanya untuk merge dua fakta atau \
             restrukturisasi total.\n\
             {budget}\
             \nKONSERVATIF: kalau ragu, JANGAN usulkan apa pun. Fakta masih relevan = biarkan.\
             \nOutput HANYA JSON array (tanpa penjelasan/code fence), maksimal {max_actions} aksi:\n\
             [{{\"action\":\"drop\",\"id\":3}},\
             {{\"action\":\"patch\",\"id\":7,\"blocks\":[{{\"search\":\"...\",\"replace\":\"...\"}}]}},\
             {{\"action\":\"rewrite\",\"id\":8,\"fact\":\"...\"}},\
             {{\"action\":\"upgrade\",\"id\":9}},\\n\
             {{\"action\":\"reclassify\",\"id\":10,\"kind\":\"preference\"}}]\n\
             Kalau tidak ada perubahan: []\n\
             \nMemory:\n{listing}"
        );
        let messages = vec![ApiMessage {
            role: "user".into(),
            content: vec![ContentBlock::Text {
                text: "Jalankan review.".into(),
            }],
        }];

        let text = call_llm_text_opts(
            agent,
            &system,
            &messages,
            &CallOpts {
                max_tokens: Some(DREAM_MAX_TOKENS),
                effort: Some("low".into()),
            },
        )
        .await
        .context("dream LLM call")?;
        let actions = parse_json_array(&text).context("dream parse output")?;
        let valid_ids: Vec<i64> = facts.iter().map(|f| f.id).collect();
        let actions_items = actions.as_array().map(|a| a.iter()).unwrap_or_default();
        for item in actions_items.take(max_actions) {
            let Some(id) = item["id"].as_i64() else {
                continue;
            };
            if !valid_ids.contains(&id) {
                continue; // id halusinasi — skip
            }
            let res: anyhow::Result<bool> = match item["action"].as_str().unwrap_or("") {
                "drop" => memory::delete_fact(&agent.pool, chat_id, id, "dream").await,
                "upgrade" => {
                    // Over budget: upgrade inferred→explicit MENAMBAH char explicit —
                    // tolak (prompt sudah melarang; ini hard guard utk output bandel).
                    if over_budget {
                        tracing::warn!(
                            chat_id,
                            id,
                            "dream upgrade ditolak — explicit over budget"
                        );
                        continue;
                    }
                    memory::set_fact_type(&agent.pool, chat_id, id, "explicit", "dream").await
                }
                "reclassify" => {
                    let kind = item["kind"].as_str().unwrap_or("").trim().to_string();
                    if !memory::valid_kind(&kind) {
                        tracing::warn!(chat_id, id, kind = %kind, "dream reclassify: kind invalid — skip");
                        continue;
                    }
                    memory::set_kind(&agent.pool, chat_id, id, &kind, "dream").await
                }
                "rewrite" => {
                    let fact = item["fact"].as_str().unwrap_or("").trim().to_string();
                    if fact.is_empty() || fact.chars().count() > MAX_FACT_CHARS {
                        continue;
                    }
                    memory::update_fact(&agent.pool, chat_id, id, &fact, "dream").await
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
                            memory::update_fact(&agent.pool, chat_id, id, &new_fact, "dream").await
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
                    "drop" => t.drops += 1,
                    "upgrade" => t.upgrades += 1,
                    "reclassify" => t.reclassifies += 1,
                    "patch" => t.patches += 1,
                    _ => t.rewrites += 1,
                },
                Ok(false) => {}
                Err(e) => tracing::warn!(chat_id, id, "dream memory: aksi gagal: {e:#}"),
            }
        }
        Ok(t)
    }
    summary.push_str(&format!(
        "🧠 memory: {drop_n} dihapus, {patch_n} di-patch, {rewrite_n} digabung/ditulis ulang, \
         {upgrade_n} inferred→explicit, {reclass_n} reclassify{} \
         explicit: {explicit_before_total} → {explicit_after_total} char / cap {}.",
        if pass_total > 1 {
            format!(" ({pass_total} pass).")
        } else {
            ". ".to_string()
        },
        memory::MAX_EXPLICIT_CHARS
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
/// pub(crate): dipakai juga session summary (OV-2) — helper bersama, pola Pilar 6.
///
/// Effort dikunci LOW: internal call = ekstraksi/konsolidasi JSON yang tidak
/// butuh reasoning panjang. Dengan effort default (z.ai: max), thinking memakan
/// seluruh budget max_tokens hingga TIDAK ada text block sama sekali — insiden
/// /dream 5 Sep (konsolidasi memory skip diam-diam tiap cycle, cap 9k tak turun).
pub(crate) async fn call_llm_text(
    agent: &Agent,
    system: &str,
    messages: &[ApiMessage],
) -> Result<String> {
    call_llm_text_opts(
        agent,
        system,
        messages,
        &CallOpts {
            max_tokens: None,
            effort: Some("low".into()),
        },
    )
    .await
}

/// Varian dengan opsi per-call (dream pakai max_tokens lebih besar — output JSON
/// aksinya panjang). Lihat CallOpts (provider/mod.rs).
pub(crate) async fn call_llm_text_opts(
    agent: &Agent,
    system: &str,
    messages: &[ApiMessage],
    opts: &CallOpts,
) -> Result<String> {
    let resp = agent.provider.chat_opts(system, messages, &[], opts).await?;
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
        // Jangan fail diam-diam lagi: sertakan stop_reason + potongan thinking —
        // gejala "budget habis di reasoning" jelas terbaca di log sejak detik nol.
        let thinking: String = resp
            .blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Thinking { thinking, .. } => Some(thinking.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        let n = thinking.chars().count();
        let snippet: String = thinking.chars().take(200).collect();
        bail!(
            "respons LLM tanpa text block (stop_reason={}; thinking {n} char{}) — \
             kemungkinan max_tokens habis di reasoning: naikkan max_tokens / turunkan effort",
            resp.stop_reason,
            if snippet.is_empty() {
                String::new()
            } else {
                format!(", potongan: {snippet:?}")
            }
        );
    }
    Ok(text)
}

/// Parse JSON array dari output LLM yang bandel: buang code fence, ambil dari
/// '[' pertama sampai ']' terakhir. Kalau utuhnya rusak / terpotong, salvage
/// objek '{...}' valid satu-satu — pola nyata glm-5.3-flash (empiris 5 Sep):
/// ']' ditutup duluan lalu aksi ditambah lagi, string fakta mengandung karakter
/// perusak JSON, atau ekor terpotong (max_tokens). Tanpa salvage, SATU bagian
/// rusak membatalkan seluruh 30+ aksi valid dan fase memory di-skip diam-diam.
fn parse_json_array(text: &str) -> Result<Value> {
    let cleaned = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```");
    let cleaned = cleaned.trim_end_matches("```").trim();
    let Some(start) = cleaned.find('[') else {
        bail!("tidak ada '[' dalam output");
    };
    let salvage = || -> Option<Value> {
        let salvaged = salvage_objects(&cleaned[start..]);
        if salvaged.is_empty() {
            return None;
        }
        tracing::warn!(valid = salvaged.len(), "parse: JSON rusak — salvage objek valid");
        Some(Value::Array(salvaged))
    };
    let slice = match cleaned.rfind(']') {
        Some(end) if end > start => &cleaned[start..=end],
        // Tanpa ']' penutup (output terpotong) — salvage objek yang sudah lengkap.
        _ => match salvage() {
            Some(v) => return Ok(v),
            None => bail!("tidak ada ']' dalam output"),
        },
    };
    match serde_json::from_str(slice) {
        Ok(v) => Ok(v),
        Err(e) => match salvage() {
            Some(v) => Ok(v),
            None => Err(e).with_context(|| {
                format!("JSON tidak valid: {}", &slice[..slice.len().min(200)])
            }),
        },
    }
}

/// Ekstrak objek '{...}' top-level (brace-balanced & string-aware) dari teks,
/// parse satu-satu, buang yang gagal — fail-soft per objek, bukan all-or-nothing.
fn salvage_objects(text: &str) -> Vec<Value> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{' {
            let (mut depth, mut in_str, mut esc) = (0usize, false, false);
            let mut j = i;
            while j < chars.len() {
                let c = chars[j];
                if in_str {
                    if esc {
                        esc = false;
                    } else if c == '\\' {
                        esc = true;
                    } else if c == '"' {
                        in_str = false;
                    }
                } else if c == '"' {
                    in_str = true;
                } else if c == '{' {
                    depth += 1;
                } else if c == '}' {
                    depth -= 1;
                    if depth == 0 {
                        let s: String = chars[i..=j].iter().collect();
                        if let Ok(v) = serde_json::from_str::<Value>(&s) {
                            out.push(v);
                        }
                        break;
                    }
                }
                j += 1;
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
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
    #[serde(default)]
    kind: Option<String>,
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
    fn parse_json_array_salvage_premature_close() {
        // Pola nyata glm-5.3-flash (empiris 5 Sep): ']' ditutup duluan, aksi
        // tambahan menyusul, ekor terpotong. Strict gagal → salvage objek valid.
        let raw = "[{\"action\":\"drop\",\"id\":1}, oops {\"action\":\"drop\",\"id\":2}]";
        let v = parse_json_array(raw).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[1]["id"], 2);
    }

    #[test]
    fn parse_json_array_salvage_truncated_tail() {
        // Terpotong tanpa ']' — objek lengkap tetap diselamatkan, yang setengah dibuang.
        let raw = "[{\"action\":\"drop\",\"id\":1},{\"action\":\"drop\",\"id\":2},{\"action\":\"rewrite\",\"id\":3,\"fact\":\"te";
        let v = parse_json_array(raw).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 2); // objek terpotong dibuang
    }

    #[test]
    fn salvage_objects_nested_braces() {
        // blocks:[{...}] bertingkat tidak boleh terpecah jadi objek terpisah.
        let objs = salvage_objects("[{\"action\":\"patch\",\"id\":7,\"blocks\":[{\"search\":\"a\",\"replace\":\"b\"}],\"x\":1}]");
        assert_eq!(objs.len(), 1);
        assert!(objs[0]["blocks"].is_array());
    }

    #[test]
    fn budget_pressure_thresholds() {
        let cap = memory::MAX_EXPLICIT_CHARS;
        assert!(budget_pressure(100).is_none());
        assert!(budget_pressure(cap * 84 / 100).is_none());
        // Kondisi insiden 5 Sep: 8973/9000 (99.7%) — WAJIB tekanan budget.
        let p = budget_pressure(8973).expect("≥85% cap harus ikut tekanan budget");
        assert!(p.contains("8973/9000"));
        assert!(p.contains("minimal 1773 char")); // 8973 - 7200: sukses cycle terukur
        assert!(p.contains("≈ 9 baris penuh")); // 1773 char ≈ 9 baris — bisa dihitung model
        assert!(p.contains("HANYA usulkan aksi atas baris (explicit)"));
        assert!(p.contains("JANGAN drop/patch/reclassify baris (inferred)"));
        assert!(p.contains("JANGAN upgrade inferred→explicit"));
        assert!(p.contains("ABAIKAN")); // hotness explicit = artefak, bukan sinyal
        assert!(budget_pressure(cap).is_some());
    }

    #[test]
    fn duplicate_detection() {
        let existing = vec![memory::MemoryFact {
            id: 1,
            fact: "Owner suka deploy di malam hari".into(),
            fact_type: "explicit".into(),
            kind: "preference".into(),
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
            r#"[{"action":"drop","id":3},{"action":"rewrite","id":7,"fact":"baru"},{"action":"upgrade","id":9},{"action":"patch","id":12,"blocks":[{"search":"lama","replace":"baru"}]},{"action":"reclassify","id":14,"kind":"entity"}]"#,
        )
        .unwrap();
        let acts: Vec<_DreamActionShape> = serde_json::from_value(v).unwrap();
        assert_eq!(acts.len(), 5);
        assert_eq!(acts[0].action, "drop");
        assert_eq!(acts[1].id, Some(7));
        assert_eq!(acts[2].action, "upgrade");
        assert_eq!(acts[3].action, "patch");
        assert_eq!(acts[3].blocks.as_ref().unwrap()[0].search, "lama");
        assert_eq!(acts[4].action, "reclassify");
        assert_eq!(acts[4].kind.as_deref(), Some("entity"));
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
            &[pb("10K", "21K"), pb("6:45/km", "6:20/km")],
        )
        .unwrap();
        assert_eq!(out, "Race 21K 1 Jan, pace 6:20/km");
    }

    #[test]
    fn patch_ambiguous_exact_rejected() {
        // SEARCH muncul 2x → ambigu → ditolak (fail-closed).
        let err = apply_patch(
            "Tagihan Rp1.234.567 dan bonus Rp1.234.567 tahunan",
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
            &[pb("10K", "21K"), pb("tidak ada string ini", "x")],
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
