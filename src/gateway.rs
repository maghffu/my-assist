use crate::agent::Agent;
use crate::notify::Notifier;
use crate::shell::ConfirmVerdict;
use crate::{memory, ocr, reminders, review, shell, skills};
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use std::sync::Arc;
use std::time::Duration;
use teloxide::prelude::*;
use teloxide::types::{BotCommand, CallbackQuery, ChatId};

/// Pencegah "ngomong sama tembok" versi ekstrem: batas total satu turn. Kalau
/// semuanya hang (provider, shell, konfirmasi tidak dijawab), turn dibunuh dan
/// owner TETAP dapat kabar — tidak pernah diem selamanya.
const TURN_TIMEOUT: Duration = Duration::from_secs(900);

const HELP_TEXT: &str = "🤖 **Hermes-Lite**\n\
\n\
Chat langsung aja — aku inget konteks & fakta penting tentang kamu. Saat aku \
kerja (run command, search, dll.), muncul pesan progres tiap langkah.\n\
\n\
**Perintah:**\n\
/status — uptime, provider, model\n\
/memory — lihat fakta yang kupelajari (/memory del <id> untuk hapus)\n\
/reminders — daftar pengingat (/reminders del <id> untuk hapus)\n\
/skills — library skill prosedural\n\
/dream — jalankan konsolidasi memory+skills sekarang\n\
/provider — info AI provider aktif\n\
/usage — pemakaian token proses ini\n\
/help — tulisan ini";

pub async fn run(bot: Bot, agent: Arc<Agent>) -> Result<()> {
    // Sinkron daftar command di menu "/" Telegram — token bot bisa dipakai ulang dari
    // aplikasi lain, jadi daftar lama (warisan) dioverwrite total saat startup.
    let menu = vec![
        BotCommand::new("help", "bantuan / daftar perintah"),
        BotCommand::new("status", "uptime, provider, model"),
        BotCommand::new("memory", "fakta yang dipelajari (/memory del <id>)"),
        BotCommand::new("reminders", "daftar pengingat (/reminders del <id>)"),
        BotCommand::new("provider", "info AI provider aktif"),
        BotCommand::new("usage", "pemakaian token proses ini"),
        BotCommand::new("skills", "library skill prosedural"),
        BotCommand::new("dream", "konsolidasi memory+skills sekarang"),
    ];
    if let Err(e) = bot.set_my_commands(menu).await {
        tracing::warn!("gagal sinkron daftar command: {:#}", e);
    }

    // Reminder trigger loop — polling tiap 30 detik (Pilar 4)
    {
        let bot = bot.clone();
        let agent = agent.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                if let Err(e) = process_due_reminders(&bot, &agent).await {
                    tracing::error!("reminder loop error: {:#}", e);
                }
            }
        });
    }

    // Dreaming cycle — berkala mingguan (Pilar 6). First tick dijadwalkan +7 hari,
    // BUKAN immediately (tokio interval default nembak langsung — boros token).
    {
        let agent = agent.clone();
        tokio::spawn(async move {
            let period = Duration::from_secs(7 * 24 * 3600);
            let mut interval = tokio::time::interval_at(
                tokio::time::Instant::now() + period,
                period,
            );
            loop {
                interval.tick().await;
                tracing::info!("dreaming cycle mingguan terpicu");
                match review::run_dream(&agent).await {
                    Ok(_) => {} // ringkasan sudah di-log run_dream
                    Err(e) => tracing::error!("dreaming cycle error: {e:#}"),
                }
            }
        });
    }

    // Dispatcher dua jalur: pesan (chat/slash command) + callback query
    // (tombol Approve/Deny confirmation gate run_command — Pilar 9 keamanan #3).
    let msg_agent = agent.clone();
    let cb_agent = agent.clone();
    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(move |bot: Bot, msg: Message| {
            let agent = msg_agent.clone();
            async move {
                if let Err(e) = handle_message(bot.clone(), msg, agent).await {
                    tracing::error!("message handler error: {:#}", e);
                }
                respond(())
            }
        }))
        .branch(
            Update::filter_callback_query().endpoint(move |bot: Bot, q: CallbackQuery| {
                let agent = cb_agent.clone();
                async move {
                    if let Err(e) = handle_callback(bot.clone(), q, agent).await {
                        tracing::error!("callback handler error: {:#}", e);
                    }
                    respond(())
                }
            }),
        );

    Dispatcher::builder(bot, handler)
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

/// Handler tombol konfirmasi destructive command: "hmc:<id>:ok|no" (Pilar 9).
/// Ambil oneshot sender yang menunggu di ShellCtx, kirim verdict, beri feedback visual.
async fn handle_callback(bot: Bot, q: CallbackQuery, agent: Arc<Agent>) -> Result<()> {
    // Keyboard konfirmasi selalu ada di chat owner; chat di luar allowlist di-drop.
    let Some(msg) = q.message.as_ref() else {
        let _ = bot.answer_callback_query(&q.id).await;
        return Ok(());
    };
    if !agent.cfg.allowed_chat_ids.contains(&msg.chat().id.0) {
        tracing::warn!(chat = %msg.chat().id.0, "callback dari chat di luar allowlist — di-drop");
        return Ok(());
    }

    match q.data.as_deref().and_then(shell::parse_confirm) {
        Some((id, verdict)) => match agent.shell.take_pending(id) {
            Some(sender) => {
                // Kirim verdict ke run_command yang sedang menunggu (oneshot).
                let _ = sender.send(verdict);
                let text = match verdict {
                    ConfirmVerdict::Allow => "✅ Approved (sekali) — melanjutkan eksekusi…",
                    ConfirmVerdict::Always => {
                        "🔁 Approved (sesi ini) — command sama tidak akan ditanya lagi sampai bot restart."
                    }
                    ConfirmVerdict::Deny => "❌ Denied — perintah dibatalkan.",
                };
                let _ = bot
                    .edit_message_text(msg.chat().id, msg.id(), text)
                    .await;
                let _ = bot.answer_callback_query(&q.id).await;
            }
            None => {
                // Sudah dijawab / timeout — hentikan spinner tombol saja.
                let _ = bot
                    .answer_callback_query(&q.id)
                    .text("⏰ Konfirmasi sudah kedaluwarsa.")
                    .await;
            }
        },
        None => {
            let _ = bot.answer_callback_query(&q.id).await;
        }
    }
    Ok(())
}

async fn handle_message(bot: Bot, msg: Message, agent: Arc<Agent>) -> Result<()> {
    let chat_id = msg.chat.id.0;

    // Hard allowlist (Pilar 9 keamanan #1): drop diam-diam, tanpa balasan.
    if !agent.cfg.allowed_chat_ids.contains(&chat_id) {
        tracing::warn!(chat_id, "pesan dari chat di luar allowlist — di-drop");
        return Ok(());
    }

    // Foto → OCR → turn agent (Pilar 7) — sebelum branch teks (foto tak punya text).
    if let Some(photos) = msg.photo() {
        return handle_photo(&bot, &msg, &agent, photos).await;
    }

    let text = match msg.text() {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        // Tipe pesan lain (dokumen, voice, sticker, dsb.) TIDAK di-drop diam —
        // owner selalu dapat kabar (dulu: "ngomong sama tembok").
        _ => {
            bot.send_message(
                msg.chat.id,
                "🤔 Aku belum bisa memproses tipe pesan ini (dokumen/voice/sticker/dll.).\n\
                 Kirim aja isinya sebagai teks, atau screenshot sebagai foto (aku bisa OCR).",
            )
            .await?;
            return Ok(());
        }
    };

    // Notifier: typing indicator berkelanjutan + pesan progres per tool call.
    let notifier = Notifier::start(bot.clone(), msg.chat.id);

    let outcome: Result<String> = if text.starts_with('/') {
        handle_command(&agent, chat_id, &text).await
    } else {
        match tokio::time::timeout(
            TURN_TIMEOUT,
            agent.run_turn(chat_id, &text, true, Some(&notifier)),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => Err(anyhow!(
                "turn melebihi batas {} detik — provider kemungkinan lambat/hang",
                TURN_TIMEOUT.as_secs()
            )),
        }
    };

    match outcome {
        Ok(reply) => {
            // Background review pasca-turn (Pilar 6): fire-and-forget, tanpa latency.
            review::spawn_post_turn_review(agent.clone(), chat_id, text.clone(), reply.clone());
            // Hapus pesan progres — balasan final yang mewakili hasil.
            notifier.finish(None).await;
            send_long(&bot, msg.chat.id, &reply).await?;
        }
        Err(e) => {
            tracing::error!(chat_id, "turn error: {:#}", e);
            notifier.finish(Some("❌ turn gagal".into())).await;
            let cause = root_cause(&e);
            bot.send_message(
                msg.chat.id,
                format!(
                    "⚠️ Ada kendala: {cause}\n\nCoba kirim ulang. Kalau terus terjadi, cek \
                     /status atau log server (`journalctl -u hermes-lite -n 50`)."
                ),
            )
            .await?;
        }
    }
    Ok(())
}

/// Ambil akar penyebab dari chain anyhow (`a: b: c` → `c`) — pesan error yang
/// langsung ke inti masalah, bukan bertumpuk menggurui.
fn root_cause(e: &anyhow::Error) -> String {
    let s = format!("{e:#}");
    let s = s
        .rsplit_once(": ")
        .map(|(_, r)| r.trim().to_string())
        .unwrap_or(s);
    s.chars().take(300).collect()
}

/// Foto dari owner: download resolusi terbesar → OCR Tesseract lokal → teks jadi
/// prompt biasa → turn agent penuh (tools tersedia). Kualitas OCR bergantung kualitas
/// gambar (Pilar 7 trade-off yang disadari).
async fn handle_photo(
    bot: &Bot,
    msg: &Message,
    agent: &Arc<Agent>,
    photos: &[teloxide::types::PhotoSize],
) -> Result<()> {
    let chat_id = msg.chat.id.0;
    let notifier = Notifier::start(bot.clone(), msg.chat.id);

    let largest = ocr::largest_photo(photos);
    notifier.notify("📥 mengunduh foto…");
    let bytes = ocr::download_photo(bot, largest).await?;
    tracing::info!(chat_id, bytes = bytes.len(), "foto diterima — OCR");

    notifier.notify("🔍 OCR — membaca teks dari foto…");
    let text =
        match ocr::extract_text(bytes, &agent.cfg.ocr_lang, agent.cfg.ocr_tessdata.as_deref()).await {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(chat_id, "OCR gagal: {e:#}");
                notifier.finish(Some(format!("❌ OCR gagal: {}", root_cause(&e)))).await;
                let m: String = format!("⚠️ OCR gagal: {e:#}").chars().take(400).collect();
                bot.send_message(msg.chat.id, m).await?;
                return Ok(());
            }
        };

    // Nggak nemu teks → jawab langsung tanpa LLM call (murah & deterministik)
    if text.chars().count() < 12 {
        notifier.finish(None).await;
        bot.send_message(
            msg.chat.id,
            "🤷 OCR tidak menemukan teks yang terbaca dari foto ini. Coba foto lebih \
             tegak/terang, atau ketik isinya langsung.",
        )
        .await?;
        return Ok(());
    }

    let prompt = ocr::build_prompt(msg.caption(), &text);
    notifier.notify("🧠 memproses teks foto…");
    let turn = tokio::time::timeout(
        TURN_TIMEOUT,
        agent.run_turn(chat_id, &prompt, true, Some(&notifier)),
    )
    .await;
    match turn {
        Ok(Ok(reply)) => {
            notifier.finish(None).await;
            review::spawn_post_turn_review(agent.clone(), chat_id, prompt, reply.clone());
            send_long(bot, msg.chat.id, &reply).await?;
        }
        Ok(Err(e)) => {
            tracing::error!(chat_id, "photo turn error: {:#}", e);
            notifier.finish(Some("❌ gagal memproses foto".into())).await;
            let cause = root_cause(&e);
            bot.send_message(
                msg.chat.id,
                format!("⚠️ Ada kendala: {cause}\n\nCoba kirim ulang fotonya."),
            )
            .await?;
        }
        Err(_) => {
            notifier.finish(Some("⏱️ timeout — turn dibatalkan".into())).await;
            bot.send_message(msg.chat.id, turn_timeout_text()).await?;
        }
    }
    Ok(())
}

fn turn_timeout_text() -> String {
    format!(
        "⏱️ Pekerjaan ini melebihi batas {} menit jadi kuhentikan — biasanya provider \
         lagi lambat/hang. Coba kirim ulang, atau pecah jadi tugas yang lebih kecil.",
        TURN_TIMEOUT.as_secs() / 60
    )
}

/// Slash commands — diproses langsung di gateway TANPA lewat LLM (murah & deterministik).
async fn handle_command(agent: &Agent, chat_id: i64, text: &str) -> Result<String> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    let cmd = parts.first().copied().unwrap_or("");

    match cmd {
        "/start" | "/help" => Ok(HELP_TEXT.into()),

        "/status" => Ok(format!(
            "🟢 **Hermes-Lite**\nuptime: {:.0}s\nprovider: `{}`\nmodel: `{}`\nsearch: `{}`{}\ncontext: {} pesan terakhir\ndb: terhubung ✅",
            agent.started.elapsed().as_secs_f64(),
            agent.provider.name(),
            agent.provider.model_name(),
            agent.cfg.search_provider,
            if agent.cfg.tavily_api_key.is_some() { "" } else { " (⚠️ tanpa TAVILY_API_KEY)" },
            agent.cfg.n_context,
        )),

        "/memory" => {
            if parts.len() >= 3 && parts[1] == "del" {
                let id: i64 = parts[2]
                    .parse()
                    .context("id harus angka — contoh: /memory del 42")?;
                let ok = memory::delete_fact(&agent.pool, chat_id, id).await?;
                return Ok(if ok {
                    format!("🗑️ Memory #{} dihapus", id)
                } else {
                    format!("Memory #{} tidak ditemukan", id)
                });
            }
            let facts = memory::list_facts(&agent.pool, chat_id, 20).await?;
            if facts.is_empty() {
                return Ok("(belum ada memory tersimpan)".into());
            }
            let body = facts
                .iter()
                .map(|f| format!("[{}] ({}) {}", f.id, f.fact_type, f.fact))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(format!(
                "🧠 **Memory** ({} terakhir):\n{}\n\nhapus: /memory del <id>",
                facts.len(),
                body
            ))
        }

        "/reminders" => {
            if parts.len() >= 3 && parts[1] == "del" {
                let id: i64 = parts[2]
                    .parse()
                    .context("id harus angka — contoh: /reminders del 7")?;
                let ok = reminders::delete(&agent.pool, chat_id, id).await?;
                return Ok(if ok {
                    format!("🗑️ Reminder #{} dihapus", id)
                } else {
                    format!("Reminder #{} tidak ditemukan", id)
                });
            }
            let list = reminders::list_pending(&agent.pool, chat_id, 10).await?;
            if list.is_empty() {
                return Ok("(tidak ada reminder pending)".into());
            }
            let body = list
                .iter()
                .map(|r| {
                    format!(
                        "[{}] {} — {} [{}{}]",
                        r.id,
                        reminders::fmt_jakarta(r.remind_at),
                        r.message,
                        r.kind,
                        r.recur
                            .as_deref()
                            .map(|c| format!(", {}", c))
                            .unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            Ok(format!(
                "⏰ **Reminders** ({} pending):\n{}\n\nhapus: /reminders del <id>",
                list.len(),
                body
            ))
        }

        "/provider" => Ok(format!(
            "🔌 Provider aktif: `{}` ({})\nSwitching runtime antar provider menyusul \
             bersama impl provider berikutnya (ROADMAP Fase 5+). Set via env `AI_PROVIDER`.",
            agent.provider.name(),
            agent.provider.model_name()
        )),

        "/usage" => {
            let u = agent.usage.lock().unwrap();
            Ok(format!(
                "📊 Token usage (proses ini, reset saat restart):\ninput: {}\noutput: {}\nturns: {}",
                u.input_tokens, u.output_tokens, u.turns
            ))
        }

        "/skills" => {
            let dir = std::path::Path::new(&agent.cfg.skills_dir);
            let metas = skills::list_skills(dir);
            if metas.is_empty() {
                return Ok(
                    "📚 Belum ada skill — terisi otomatis saat agent menyelesaikan masalah \
                     non-trivial (bisa juga minta dia simpan manual)."
                        .into(),
                );
            }
            let body = metas
                .iter()
                .map(|s| format!("- {} ({} KB)", s.name, (s.bytes + 1023) / 1024))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(format!(
                "📚 **Skills** ({} file, di `{}`):\n{}",
                metas.len(),
                agent.cfg.skills_dir,
                body
            ))
        }

        "/dream" => {
            // Konsolidasi manual (loop mingguan jalan otomatis; ini utk test/darurat)
            let summary = review::run_dream(agent).await?;
            Ok(format!("💤 Dream selesai:\n{}", summary))
        }

        other => Ok(format!(
            "Perintah {} tidak dikenal — ketik /help.",
            other
        )),
    }
}

/// Reminder loop: kirim static reminder / eksekusi job / reschedule recurring (Pilar 4).
///
/// Bugfix retry-forever: kalau pengiriman gagal (mis. AI rate-limit/auth error),
/// TIDAK lagi `continue` tanpa tindakan (yang membuat remind_at tertinggal di masa
/// lalu & retry tiap 30 detik membabi buta). Sekarang gagal memicu backoff
/// eksponensial via `bump_fail`, sehingga job/reminder tetap bisa dicoba lagi tapi
/// intervalnya makin jarang, dan one-shot menyerah setelah kumulatif > 24 jam.
async fn process_due_reminders(bot: &Bot, agent: &Agent) -> Result<()> {
    for r in reminders::due_now(&agent.pool).await? {
        tracing::info!(reminder_id = r.id, kind = %r.kind, "firing reminder");

        let original_remind_at = r.remind_at;
        let outcome: Result<()> = async {
            match r.kind.as_str() {
                "job" => {
                    // Job: agent mengeksekusi instruksi dengan context segar (Pilar 4).
                    // Notifier kasih visibilitas progres — job bisa jalan berhari-hari.
                    // Konvensi job prompt: balas "SKIP" = tidak ada yang perlu
                    // dikirim (mis. hari rest) — diam, tanpa spam ke owner.
                    let n = Notifier::start(bot.clone(), ChatId(r.chat_id));
                    let preview: String = r.message.chars().take(120).collect();
                    n.notify(format!("⚙️ job mulai: {preview}"));
                    let out = agent
                        .run_turn(
                            r.chat_id,
                            &format!("⚙️ [scheduled job] {}", r.message),
                            false,
                            Some(&n),
                        )
                        .await;
                    match out {
                        Ok(out) => {
                            if out.trim().eq_ignore_ascii_case("skip") {
                                n.finish(None).await;
                                tracing::info!(
                                    reminder_id = r.id,
                                    "job SKIP — tidak ada output utk owner"
                                );
                            } else {
                                n.finish(None).await;
                                send_long(
                                    bot,
                                    ChatId(r.chat_id),
                                    &format!("⚙️ Job selesai:\n{}", out),
                                )
                                .await?;
                            }
                        }
                        Err(e) => {
                            n.finish(Some(format!("❌ job gagal: {}", root_cause(&e))))
                                .await;
                            return Err(e);
                        }
                    }
                }
                _ => {
                    bot.send_message(ChatId(r.chat_id), format!("⏰ {}", r.message))
                        .await?;
                }
            }
            Ok(())
        }
        .await;

        match outcome {
            Ok(()) => {
                reminders::reset_fail(&agent.pool, r.id).await?;
                // Reschedule dari jadwal asli (bukan remind_at yang sudah mundur
                // akibat backoff) biar siklus recurring tetap konsisten.
                match r
                    .recur
                    .as_deref()
                    .and_then(|rec| reminders::compute_next_run(rec, original_remind_at))
                {
                    Some(next) => reminders::reschedule(&agent.pool, r.id, next).await?,
                    None => reminders::mark_sent(&agent.pool, r.id).await?,
                }
            }
            Err(e) => {
                tracing::error!(
                    reminder_id = r.id,
                    fail = r.fail_count.saturating_add(1),
                    "reminder gagal dikirim: {:#}",
                    e
                );
                // Backoff — JANGAN coba lagi tiap 30 detik (bug retry-forever).
                let new_fail =
                    reminders::bump_fail(&agent.pool, r.id, Utc::now(), r.fail_count).await?;
                // One-shot yang sudah gagal berkali-kali (kumulatif backoff > 24 jam):
                // menyerah & tandai sent supaya tidak selamanya dipukul oleh loop.
                if r.recur.is_none()
                    && reminders::cumulative_backoff_secs(new_fail) > reminders::GIVEUP_AFTER_SECS
                {
                    reminders::mark_sent(&agent.pool, r.id).await?;
                    tracing::warn!(
                        reminder_id = r.id,
                        "one-shot reminder menyerah setelah backoff berkali-kali"
                    );
                }
            }
        }
    }
    Ok(())
}

/// Telegram limit 4096 char/pesan — chunk 3800 dengan backtrack ke newline.
async fn send_long(bot: &Bot, chat_id: ChatId, text: &str) -> Result<()> {
    for chunk in split_message(text) {
        bot.send_message(chat_id, chunk).await?;
    }
    Ok(())
}

pub fn split_message(text: &str) -> Vec<String> {
    const MAX: usize = 3800;
    if text.chars().count() <= MAX {
        return vec![text.to_string()];
    }

    let mut out = Vec::new();
    let mut rest = text;
    while rest.chars().count() > MAX {
        let mut cut = rest
            .char_indices()
            .nth(MAX)
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        // backtrack ke newline biar chunk tidak motong di tengah kalimat
        if let Some(pos) = rest[..cut].rfind('\n') {
            if pos > MAX / 2 {
                cut = pos + 1;
            }
        }
        let (chunk, remainder) = rest.split_at(cut);
        out.push(chunk.to_string());
        rest = remainder;
    }
    if !rest.is_empty() {
        out.push(rest.to_string());
    }
    out
}
