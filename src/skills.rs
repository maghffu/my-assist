//! Skills (Pilar 11, ROADMAP Fase 6): pengetahuan prosedural — file markdown di
//! direktori `skills/`, satu file per skill. Komplementer memory (fakta tentang
//! owner): skills menyimpan CARA mengerjakan sesuatu (langkah, command yang
//! terbukti jalan, gotchas).
//!
//! Storage file (bukan tabel): jumlah skill single-user kecil, human-readable,
//! bisa diedit manual seperti `soul.md`, git-able — zero-migration (AGENTS.md).
//!
//! Injection ke system prompt: daftar nama SELALU dimuat; skill yang cocok
//! keyword dengan pesan dimuat penuh (matching sederhana by nama dulu — jangan
//! buru-buru ke semantic matching, lihat Prinsip Retrieval).

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Batas ukuran satu file skill (byte).
const MAX_SKILL_BYTES: u64 = 20_000;
/// Konten skill yang di-inject ke system prompt dipotong di sini (char).
const INJECT_MAX_CHARS: usize = 4_000;
/// Maksimum skill yang di-inject penuh per turn.
const INJECT_MAX_FILES: usize = 3;
/// Token keyword minimal sepanjang ini biar tidak false-positive ("a", "di").
const MIN_TOKEN_LEN: usize = 3;

pub struct SkillMeta {
    pub name: String,     // "renew-ssl-nginx"
    pub filename: String, // "renew-ssl-nginx.md"
    pub bytes: u64,
}

/// Slug dari nama bebas → nama file skill: lowercase, [a-z0-9-], collapse dash.
pub fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = true; // trim dash di awal
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug.chars().take(60).collect()
}

/// Simpan/overwrite skill. Validasi anti-trivial (Pilar 11: jangan simpan FAQ/one-liner).
pub fn save_skill(dir: &Path, name: &str, content: &str) -> Result<String> {
    let name = name.trim();
    let content = content.trim();
    let slug = slugify(name);
    if slug.is_empty() {
        bail!("nama skill tidak valid (harus mengandung huruf/angka): {name:?}");
    }
    if content.chars().count() < 80 {
        bail!(
            "konten terlalu pendek untuk skill (min 80 char). Skill = pengetahuan prosedural \
             non-trivial (langkah + command + gotchas) — JANGAN simpan jawaban FAQ/one-liner."
        );
    }
    if content.len() > MAX_SKILL_BYTES as usize {
        bail!("konten > {MAX_SKILL_BYTES} byte — pecah atau diringkas");
    }
    fs::create_dir_all(dir).with_context(|| format!("gagal buat dir {}", dir.display()))?;
    let path = dir.join(format!("{slug}.md"));
    let existed = path.exists();
    fs::write(&path, content).with_context(|| format!("gagal tulis {}", path.display()))?;
    tracing::info!(skill = %slug, updated = existed, bytes = content.len(), "save_skill");
    Ok(if existed {
        format!("✅ Skill `{slug}` di-update ({} char).", content.chars().count())
    } else {
        format!("✅ Skill baru `{slug}` tersimpan ({} char).", content.chars().count())
    })
}

/// Hapus file skill by nama slug — dipakai dreaming cycle.
pub fn delete_skill(dir: &Path, filename: &str) -> Result<bool> {
    // guard: hanya nama file polos di dalam dir (tanpa path traversal)
    if filename.contains('/') || filename.contains('\\') || !filename.ends_with(".md") {
        bail!("nama file skill tidak valid: {filename:?}");
    }
    let path = dir.join(filename);
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path).with_context(|| format!("gagal hapus {}", path.display()))?;
    Ok(true)
}

/// Tulis ulang isi skill — dipakai dreaming cycle (merge/update).
pub fn rewrite_skill(dir: &Path, filename: &str, content: &str) -> Result<()> {
    if filename.contains('/') || filename.contains('\\') || !filename.ends_with(".md") {
        bail!("nama file skill tidak valid: {filename:?}");
    }
    let path = dir.join(filename);
    if !path.exists() {
        bail!("skill {filename} tidak ditemukan");
    }
    fs::write(&path, content).with_context(|| format!("gagal tulis {}", path.display()))?;
    Ok(())
}

/// Daftar semua skill (urut nama). Direktori belum ada → kosong.
pub fn list_skills(dir: &Path) -> Vec<SkillMeta> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<SkillMeta> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| !n.starts_with('.'))
        })
        .filter_map(|e| {
            let filename = e.file_name().to_string_lossy().to_string();
            let bytes = e.metadata().ok().map(|m| m.len()).unwrap_or(0);
            Some(SkillMeta {
                name: filename.strip_suffix(".md")?.to_string(),
                filename,
                bytes,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Bagian system prompt untuk skills (Pilar 11): daftar nama selalu dimuat;
/// skill yang keyword-nya cocok dengan pesan dimuat penuh (max 3 file, cap 4K char).
pub fn section_for_prompt(dir: &Path, user_text: &str) -> String {
    let skills = list_skills(dir);
    if skills.is_empty() {
        return "(belum ada skill tersimpan — gunakan save_skill setelah menyelesaikan \
                masalah non-trivial)"
            .into();
    }

    let names = skills
        .iter()
        .map(|s| s.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    let text_lower = user_text.to_lowercase();
    let mut matched: Vec<&SkillMeta> = skills
        .iter()
        .filter(|s| {
            s.name
                .split('-')
                .filter(|t| t.len() >= MIN_TOKEN_LEN)
                .any(|t| text_lower.contains(t))
        })
        .take(INJECT_MAX_FILES)
        .collect();

    let mut out = format!(
        "Tersedia: {names} — muat penuh via `read_file skills/<nama>.md` kalau relevan \
         tapi belum dimuat di bawah."
    );

    if matched.is_empty() {
        return out;
    }
    out.push_str("\n\nSkill relevan dengan pesan ini:\n");
    // sortir by ukuran naik — skill kecil (lebih spesifik) pasti masuk
    matched.sort_by_key(|s| s.bytes);
    for s in matched.iter().take(INJECT_MAX_FILES) {
        let content = fs::read_to_string(dir.join(&s.filename)).unwrap_or_default();
        let capped: String = content.chars().take(INJECT_MAX_CHARS).collect();
        out.push_str(&format!(
            "\n### skills/{}\n{}\n",
            s.filename,
            capped.trim_end()
        ));
    }
    out
}

/// Path absolut skills dir dari config (dibuat kalau belum ada) — dipakai owner/read_file.
pub fn ensure_dir(dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(dir)?;
    Ok(dir.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_names() {
        assert_eq!(slugify("Renew SSL (nginx)"), "renew-ssl-nginx");
        assert_eq!(slugify("  deploy -- docker!!  "), "deploy-docker");
        assert_eq!(slugify("Cek Backup Mingguan"), "cek-backup-mingguan");
        assert_eq!(slugify("!!!"), "");
        assert_eq!(slugify("écc"), "cc");
    }

    #[test]
    fn save_list_delete_roundtrip() {
        let dir = std::env::temp_dir().join(format!("hermes-skills-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        // anti-trivial
        assert!(save_skill(&dir, "x", "pendek").is_err());
        let ok = save_skill(
            &dir,
            "Renew SSL (nginx)",
            "## Langkah\n1. certbot renew\n2. test config\ngotcha: port 80 dipakai apache — stop dulu",
        )
        .unwrap();
        assert!(ok.contains("renew-ssl-nginx"));
        assert_eq!(list_skills(&dir).len(), 1);
        assert_eq!(list_skills(&dir)[0].name, "renew-ssl-nginx");

        // overwrite = update
        save_skill(&dir, "renew-ssl   nginx!!", "versi dua yang cukup panjang untuk lolos validasi minimal");
        assert_eq!(list_skills(&dir).len(), 1, "slug sama → satu file");

        assert!(delete_skill(&dir, "renew-ssl-nginx.md").unwrap());
        assert!(list_skills(&dir).is_empty());
        assert!(!delete_skill(&dir, "renew-ssl-nginx.md").unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn path_traversal_guard() {
        let dir = Path::new("/tmp/nowhere-xyz");
        assert!(delete_skill(dir, "../soul.md").is_err());
        assert!(delete_skill(dir, "sub/x.md").is_err());
        assert!(rewrite_skill(dir, "x.txt", "c").is_err());
    }

    #[test]
    fn inject_matches_by_keyword() {
        let dir = std::env::temp_dir().join(format!("hermes-skills-inj-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let body = "## Langkah\n1. login ke server via ssh\n2. jalankan perintah utama\n3. verifikasi hasilnya\n\ngotcha: port 80 sering dipakai apache — matikan dulu sebelum renew.";
        save_skill(&dir, "renew-ssl-nginx", body).unwrap();
        save_skill(&dir, "deploy-docker-vps", body).unwrap();

        let hit = section_for_prompt(&dir, "gimana cara renew sertifikat SSL di server ini?");
        assert!(hit.contains("Skill relevan"), "keyword match: {hit}");
        assert!(hit.contains("### skills/renew-ssl-nginx.md"));
        // nama semua skill selalu ada di daftar, tapi konten penuh yang tidak cocok tidak dimuat
        assert!(!hit.contains("### skills/deploy-docker"));

        let miss = section_for_prompt(&dir, "halo, apa kabar?");
        assert!(miss.contains("Tersedia: deploy-docker-vps, renew-ssl-nginx"), "daftar nama selalu dimuat: {miss}");
        assert!(!miss.contains("###"));

        // token pendek (<3 char) tidak dipakai matching — "vps" cocok keduanya, "di" tidak
        assert!(section_for_prompt(&dir, "cek vps").contains("deploy-docker"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_dir_section() {
        let s = section_for_prompt(Path::new("/tmp/definitely-no-skills-here"), "apa saja");
        assert!(s.contains("belum ada skill"));
    }
}
