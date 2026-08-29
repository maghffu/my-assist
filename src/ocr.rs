//! OCR (Pilar 7, ROADMAP Fase 7): foto Telegram → Tesseract lokal → teks polos.
//!
//! Output teks = input universal — kompatibel dengan provider manapun, vision-capable
//! atau tidak (AGENTS.md Pilar 8). Kualitas bergantung kualitas gambar input (foto
//! miring/gelap/tulisan tangan hasilnya lebih jelek) — trade-off yang disadari.
//!
//! Implementasi: leptess (binding native libtesseract + leptonica), unix-only.
//! Proses OCR CPU-bound → `spawn_blocking`; init per-call (single-user, init ±100ms).
//! Binding di non-unix: stub error (dev Windows — binary tetap buildable).

use anyhow::{bail, Context, Result};
use teloxide::net::Download;
use teloxide::prelude::*;
use teloxide::types::PhotoSize;

/// Batas ukuran foto yang di-download dari Telegram (paling besar pun ~1-2MB).
const MAX_PHOTO_BYTES: usize = 10 * 1024 * 1024;
/// Cap teks OCR yang masuk context LLM.
pub const OCR_MAX_CHARS: usize = 15_000;

/// Ambil foto resolusi terbesar dari array PhotoSize Telegram.
pub fn largest_photo<'a>(sizes: &'a [PhotoSize]) -> &'a PhotoSize {
    sizes
        .iter()
        .max_by_key(|p| (p.width, p.height))
        .expect("photo sizes tidak kosong")
}

/// Download bytes foto dari Telegram Bot API.
pub async fn download_photo(bot: &Bot, photo: &PhotoSize) -> Result<Vec<u8>> {
    let file = bot
        .get_file(&photo.file.id)
        .await
        .context("gagal get_file dari Telegram")?;
    let mut bytes = Vec::new();
    bot.download_file(&file.path, &mut bytes)
        .await
        .context("gagal download foto dari Telegram")?;
    if bytes.len() > MAX_PHOTO_BYTES {
        bail!("foto terlalu besar ({} MB > 10MB)", bytes.len() / 1024 / 1024);
    }
    Ok(bytes)
}

/// OCR bytes gambar → teks. Blocking (Tesseract native) → spawn_blocking.
#[cfg(unix)]
pub async fn extract_text(bytes: Vec<u8>, lang: &str, tessdata: Option<&str>) -> Result<String> {
    let lang = lang.to_string();
    let tessdata = tessdata.map(str::to_string);
    tokio::task::spawn_blocking(move || -> Result<String> {
        let mut lt = leptess::LepTess::new(tessdata.as_deref(), &lang).map_err(|e| {
            anyhow::anyhow!("init Tesseract gagal (cek tessdata & OCR_LANG={lang:?}): {e:?}")
        })?;
        lt.set_image_from_mem(&bytes)
            .map_err(|e| anyhow::anyhow!("gagal decode gambar: {e:?}"))?;
        let text = lt
            .get_utf8_text()
            .map_err(|e| anyhow::anyhow!("gagal ekstrak teks: {e:?}"))?;
        Ok(text.trim().to_string())
    })
    .await
    .context("OCR worker panic")?
}

/// Stub non-unix (dev Windows) — binary tetap buildable tanpa libtesseract.
#[cfg(not(unix))]
pub async fn extract_text(_bytes: Vec<u8>, _lang: &str, _tessdata: Option<&str>) -> Result<String> {
    bail!("OCR hanya tersedia di build unix (libtesseract + leptess) — jalankan di VPS");
}

/// Format teks OCR menjadi prompt untuk agent (dipakai gateway).
pub fn build_prompt(caption: Option<&str>, ocr_text: &str) -> String {
    let body: String = ocr_text.chars().take(OCR_MAX_CHARS).collect();
    let note = if ocr_text.chars().count() > OCR_MAX_CHARS {
        "\n(OCR dipotong — teks foto lebih panjang)"
    } else {
        ""
    };
    match caption.map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => format!(
            "[owner mengirim foto, caption: \"{c}\"]\nHasil OCR Tesseract:\n{body}{note}"
        ),
        None => format!("[owner mengirim foto]\nHasil OCR Tesseract:\n{body}{note}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_formatting() {
        let p = build_prompt(None, "INVOICE #42\nTotal: Rp 500.000");
        assert!(p.contains("[owner mengirim foto]"));
        assert!(p.contains("INVOICE #42"));

        let p = build_prompt(Some("tolong catat ini ya"), "rapat jam 3");
        assert!(p.contains("caption: \"tolong catat ini ya\""));
        assert!(p.contains("rapat jam 3"));

        // caption kosong/whitespace dianggap tidak ada
        assert!(build_prompt(Some("  "), "x").contains("[owner mengirim foto]"));
    }

    #[test]
    fn long_ocr_capped() {
        let s = "a".repeat(20_000);
        let p = build_prompt(None, &s);
        assert!(p.contains("dipotong"));
        assert!(p.chars().count() < 20_000);
    }

    /// Integration test OCR native (unix-only, aset statis) — bukti leptess +
    /// tessdata + leptonica berjalan end-to-end di mesin ini.
    #[cfg(all(test, unix))]
    #[tokio::test]
    async fn ocr_extracts_text_from_sample() {
        let bytes = std::fs::read("testdata/ocr-sample.png").expect("aset test ada");
        let text = extract_text(bytes, "eng", None).await.expect("OCR jalan");
        assert!(text.to_lowercase().contains("halo"), "hasil OCR: {text}");
        assert!(text.contains("12345"), "hasil OCR: {text}");
    }
}
