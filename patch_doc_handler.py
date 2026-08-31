#!/usr/bin/env python3
"""Patch: document handler di gateway.rs (terima file dari Telegram → simpan → turn agent).

Idempotent: kalau `handle_document` sudah ada di source, batal tanpa mengubah apa pun.
"""
import sys
import pathlib

P = pathlib.Path("/root/my-assist/src/gateway.rs")
src = P.read_text()

if "handle_document" in src:
    print("SKIP: sudah dipatch (handle_document ada)")
    sys.exit(0)

# ---- Anchor A: branch document setelah branch foto di handle_message ----
anchor_a = """    // Foto → OCR → turn agent (Pilar 7) — sebelum branch teks (foto tak punya text).
    if let Some(photos) = msg.photo() {
        return handle_photo(&bot, &msg, &agent, photos).await;
    }
"""
insert_a = """    // Dokumen (file attachment) → download → simpan → turn agent (pasangan Pilar 7):
    // file teks kecil di-inline ke prompt, sisanya cukup ditunjuk path-nya (agent
    // baca sendiri via read_file/run_command).
    if let Some(doc) = msg.document() {
        return handle_document(&bot, &msg, &agent, doc).await;
    }
"""
assert src.count(anchor_a) == 1, f"anchor A ketemu {src.count(anchor_a)}x"
src = src.replace(anchor_a, anchor_a + insert_a)

# ---- Anchor B: fallback message — dokumen sekarang didukung ----
old_b1 = "Aku belum bisa memproses tipe pesan ini (dokumen/voice/sticker/dll.)."
new_b1 = "Aku belum bisa memproses tipe pesan ini (voice/sticker/lokasi/dll.)."
old_b2 = "Kirim aja isinya sebagai teks, atau screenshot sebagai foto (aku bisa OCR)."
new_b2 = "Kirim teks langsung, screenshot sebagai foto, atau file sebagai dokumen."
assert src.count(old_b1) == 1, f"anchor B1 ketemu {src.count(old_b1)}x"
assert src.count(old_b2) == 1, f"anchor B2 ketemu {src.count(old_b2)}x"
src = src.replace(old_b1, new_b1).replace(old_b2, new_b2)

# ---- Anchor C: sisipkan handler + helper + tests sebelum turn_timeout_text ----
anchor_c = "fn turn_timeout_text() -> String {"
assert src.count(anchor_c) == 1, f"anchor C ketemu {src.count(anchor_c)}x"

new_code = """/// Batas ukuran dokumen (Telegram Bot API getFile maks 20MB — dijaga sama).
const MAX_DOC_BYTES: usize = 20 * 1024 * 1024;
/// Cap teks dokumen yang di-inline langsung ke prompt (di luar ini → via read_file).
const DOC_INLINE_MAX_CHARS: usize = 15_000;

/// Nama file aman untuk disimpan: buang path component & karakter aneh.
fn sanitize_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\\\']).next().unwrap_or("file");
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || "._-+ ()".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() { "file".into() } else { trimmed.to_string() }
}

/// Prompt untuk dokumen: teks UTF-8 kecil di-inline (mirip OCR), file besar/biner
/// cukup ditunjuk path-nya — agent baca via read_file / run_command.
fn build_doc_prompt(
    file_path: &str,
    name: &str,
    size_bytes: usize,
    mime: Option<&str>,
    caption: Option<&str>,
    text: Option<&str>,
) -> String {
    let kb = size_bytes / 1024;
    let meta = format!(
        "{} ({} KB{})",
        name,
        kb,
        mime.map(|m| format!(", {m}")).unwrap_or_default()
    );
    match text {
        Some(t) => {
            let body: String = t.chars().take(DOC_INLINE_MAX_CHARS).collect();
            let note = if t.chars().count() > DOC_INLINE_MAX_CHARS {
                format!("\\n(isi dipotong — file lengkap: {file_path})")
            } else {
                String::new()
            };
            match caption.map(str::trim).filter(|c| !c.is_empty()) {
                Some(c) => {
                    format!("[owner mengirim file {meta}, caption: \\"{c}\\"]\\nIsi file:\\n{body}{note}")
                }
                None => format!("[owner mengirim file {meta}]\\nIsi file:\\n{body}{note}"),
            }
        }
        None => {
            let cap = caption
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(|c| format!("\\nCaption: \\"{c}\\""))
                .unwrap_or_default();
            format!(
                "[owner mengirim file {meta}]\\nFile tersimpan di: {file_path}{cap}\\n\\
                 File ini bukan teks kecil — kalau perlu isinya, baca via read_file (teks) \\
                 atau telusuri via run_command (arsip/biner). Kerjakan permintaan owner di \\
                 caption; kalau caption kosong, ringkas isi file."
            )
        }
    }
}

/// Dokumen dari owner: download dari Telegram, simpan ke DOWNLOADS_DIR (default
/// `downloads/` — di bawah workdir /opt/hermes-lite sehingga guard read_file lolos),
/// lalu jalankan turn agent penuh (tools tersedia) — mirip alur foto → OCR.
async fn handle_document(
    bot: &Bot,
    msg: &Message,
    agent: &Arc<Agent>,
    doc: &teloxide::types::Document,
) -> Result<()> {
    let chat_id = msg.chat.id.0;
    let notifier = Notifier::start(bot.clone(), msg.chat.id);

    let raw_name = doc.file_name.clone().unwrap_or_else(|| "file".into());
    let name = sanitize_filename(&raw_name);
    let size = doc.file.size as usize;

    if size > MAX_DOC_BYTES {
        notifier.finish(Some("❌ file terlalu besar".into())).await;
        bot.send_message(
            msg.chat.id,
            format!(
                "⚠️ File {} MB melebihi batas 20MB Bot API — kirim via cara lain (mis. scp ke VPS).",
                size / 1024 / 1024
            ),
        )
        .await?;
        return Ok(());
    }

    notifier.notify("📥 mengunduh file…");
    let tg_file = bot
        .get_file(&doc.file.id)
        .await
        .context("gagal get_file dari Telegram")?;
    let mut bytes = Vec::new();
    bot.download_file(&tg_file.path, &mut bytes)
        .await
        .context("gagal download dokumen dari Telegram")?;

    let dir = std::env::var("DOWNLOADS_DIR").unwrap_or_else(|_| "downloads".into());
    tokio::fs::create_dir_all(&dir)
        .await
        .context("gagal bikin direktori downloads")?;
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let file_path = std::path::Path::new(&dir).join(format!("{stamp}_{name}"));
    tokio::fs::write(&file_path, &bytes)
        .await
        .with_context(|| format!("gagal simpan {}", file_path.display()))?;
    let file_path_str = file_path.display().to_string();
    tracing::info!(chat_id, bytes = bytes.len(), name = %name, "dokumen diterima — disimpan di {file_path_str}");

    // Teks UTF-8 kecil → inline ke prompt (hemat 1 tool call); selain itu → path.
    let inline_text = std::str::from_utf8(&bytes)
        .ok()
        .filter(|_| bytes.len() <= DOC_INLINE_MAX_CHARS * 4);

    notifier.notify("🧠 memproses file…");
    let prompt = build_doc_prompt(
        &file_path_str,
        &name,
        bytes.len(),
        doc.mime_type.as_ref().map(|m| m.as_ref()),
        msg.caption(),
        inline_text,
    );
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
            tracing::error!(chat_id, "document turn error: {:#}", e);
            notifier.finish(Some("❌ gagal memproses file".into())).await;
            let cause = root_cause(&e);
            bot.send_message(
                msg.chat.id,
                format!("⚠️ Ada kendala: {cause}\\n\\nCoba kirim ulang filenya."),
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

#[cfg(test)]
mod doc_tests {
    use super::*;

    #[test]
    fn sanitize_strips_paths_and_weird() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("a b.txt"), "a b.txt");
        assert_eq!(sanitize_filename(""), "file");
        assert_eq!(sanitize_filename("   "), "file");
        assert_eq!(sanitize_filename("résumé.pdf"), "résumé.pdf");
    }

    #[test]
    fn prompt_inline_with_caption() {
        let p = build_doc_prompt(
            "downloads/x.txt",
            "x.txt",
            2048,
            Some("text/plain"),
            Some("ringkas ini"),
            Some("halo"),
        );
        assert!(p.contains("caption: \\"ringkas ini\\""));
        assert!(p.contains("Isi file:\\nhalo"));
    }

    #[test]
    fn prompt_path_mode() {
        let p = build_doc_prompt("downloads/a.zip", "a.zip", 5 * 1024 * 1024, None, None, None);
        assert!(p.contains("read_file"));
        assert!(p.contains("downloads/a.zip"));
        assert!(p.contains("5120 KB"));
    }
}

"""
src = src.replace(anchor_c, new_code + anchor_c)
P.write_text(src)
print("OK: gateway.rs dipatch (document handler + tests)")
