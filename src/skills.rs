//! Skills (Pilar 11, ROADMAP Fase 6): pengetahuan prosedural — file markdown di
//! direktori `skills/`, satu file per skill. Komplementer memory (fakta tentang
//! owner): skills menyimpan CARA mengerjakan sesuatu (langkah, command yang
//! terbukti jalan, gotchas).
//!
//! Storage file (bukan tabel): jumlah skill single-user kecil, human-readable,
//! bisa diedit manual seperti `soul.md`, git-able — zero-migration (AGENTS.md).
//!
//! Injection ke system prompt (Pilar 11): daftar nama + L0 description selalu
//! dimuat; skill yang cocok keyword (token dari nama + description — adopsi L0
//! abstract OpenViking) dengan pesan dimuat penuh (matching sederhana by token
//! dulu — jangan buru-buru ke semantic matching, lihat Prinsip Retrieval).

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
/// Cap panjang L0 description (char) — satu kalimat, bukan paragraf.
const MAX_DESCRIPTION_CHARS: usize = 160;

/// Kata fungsi umum (ID/EN) yang tidak boleh jadi token matching dari description
/// — tanpa ini hampir semua pesan match skill mana pun.
const DESC_STOPWORDS: &[&str] = &[
    "dan", "yang", "untuk", "dengan", "dari", "atau", "juga", "adalah", "pada",
    "tentang", "ini", "itu", "apa", "gimana", "cara", "bisa", "pakai", "the",
    "and", "for", "with", "from", "this", "that", "how", "why", "using", "use",
];

/// Statistik injection skill per turn — dipakai context trace (OV-4).
#[derive(Default, Debug, Clone, Copy)]
pub struct SkillsPromptStats {
    pub listed: usize,
    pub injected: usize,
}

pub struct SkillMeta {
    pub name: String,     // "renew-ssl-nginx"
    pub filename: String, // "renew-ssl-nginx.md"
    /// L0 abstract (adopsi OpenViking): frontmatter `description:` atau fallback
    /// baris pertama konten — satu kalimat yang menjelaskan topik skill.
    pub description: String,
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

/// Parse L0 description dari isi skill (adopsi `.abstract.md` OpenViking, satu
/// field saja): kalau konten diawali frontmatter `---` cari penutupnya dan ambil
/// baris `description: ...`. Parse manual tanpa crate YAML (±20 baris — prinsip
/// low-footprint, hanya satu field).
pub fn parse_description(content: &str) -> String {
    let c = content.trim_start_matches('\u{feff}');
    if let Some(rest) = c.trim_start().strip_prefix("---") {
        // harus diikuti newline ("----" bukan pembuka frontmatter)
        let rest = rest
            .strip_prefix("\r\n")
            .or_else(|| rest.strip_prefix('\n'))
            .unwrap_or(rest);
        for line in rest.lines() {
            let t = line.trim();
            if t == "---" {
                break; // frontmatter tanpa description → fallback
            }
            if let Some(v) = t.strip_prefix("description:") {
                let v = v.trim().trim_matches('"').trim_matches('\'').trim();
                if !v.is_empty() {
                    return v.chars().take(MAX_DESCRIPTION_CHARS).collect();
                }
            }
        }
    }
    fallback_description(content)
}

/// Fallback tanpa frontmatter (skill lama otomatis dapat L0): baris pertama
/// non-kosong yang BUKAN heading — atau heading itu sendiri minus `#`.
fn fallback_description(content: &str) -> String {
    let mut in_fm = false;
    let mut line_iter = content.trim_start_matches('\u{feff}').lines();
    // skip blok frontmatter kalau ada (tanpa description → lanjut ke body)
    if content.trim_start().starts_with("---") {
        in_fm = true;
        line_iter.next();
    }
    for line in line_iter {
        let t = line.trim();
        if in_fm {
            if t == "---" {
                in_fm = false;
            }
            continue;
        }
        if t.is_empty() {
            continue;
        }
        let out = t
            .trim_start_matches('#')
            .trim()
            .trim_end_matches('#')
            .trim();
        let out = if out.is_empty() { t } else { out };
        return out.chars().take(MAX_DESCRIPTION_CHARS).collect();
    }
    String::new()
}

/// Buang blok frontmatter dari kepala konten (kalau ada) → body murni.
fn strip_frontmatter(content: &str) -> &str {
    let c = content.trim_start_matches('\u{feff}');
    let Some(rest) = c.trim_start().strip_prefix("---") else {
        return c;
    };
    let Some(rest) = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))
    else {
        return c; // "----..." bukan pembuka frontmatter
    };
    let mut offset = 0usize;
    for line in rest.split_inclusive('\n') {
        let t = line.trim_end_matches(['\n', '\r']);
        offset += line.len();
        if t == "---" {
            return &rest[offset..];
        }
    }
    c // tidak ada penutup — bukan frontmatter valid, biarkan utuh
}

/// Simpan/overwrite skill. `description` opsional (kosong → derive dari baris
/// pertama konten). Frontmatter `description:` ditulis di kepala file; overwrite
/// lama mengganti description. Validasi anti-trivial (Pilar 11: jangan simpan
/// FAQ/one-liner).
pub fn save_skill(
    dir: &Path,
    name: &str,
    description: Option<&str>,
    content: &str,
) -> Result<String> {
    let name = name.trim();
    let body = strip_frontmatter(content).trim();
    let slug = slugify(name);
    if slug.is_empty() {
        bail!("nama skill tidak valid (harus mengandung huruf/angka): {name:?}");
    }
    if body.chars().count() < 80 {
        bail!(
            "konten terlalu pendek untuk skill (min 80 char). Skill = pengetahuan prosedural \
             non-trivial (langkah + command + gotchas) — JANGAN simpan jawaban FAQ/one-liner."
        );
    }
    let desc = match description.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) if d.chars().count() > MAX_DESCRIPTION_CHARS => {
            bail!("description terlalu panjang (max {MAX_DESCRIPTION_CHARS} char, satu kalimat)");
        }
        Some(d) => d.to_string(),
        None => {
            let d = fallback_description(body);
            if d.is_empty() {
                bail!("tidak bisa menurunkan description dari konten — isi parameter description");
            }
            d
        }
    };
    let content = format!("---\ndescription: {desc}\n---\n\n{body}");
    if content.len() > MAX_SKILL_BYTES as usize {
        bail!("konten > {MAX_SKILL_BYTES} byte — pecah atau diringkas");
    }
    fs::create_dir_all(dir).with_context(|| format!("gagal buat dir {}", dir.display()))?;
    let path = dir.join(format!("{slug}.md"));
    let existed = path.exists();
    fs::write(&path, &content).with_context(|| format!("gagal tulis {}", path.display()))?;
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
/// Safety net L0: kalau konten hasil rewrite tidak punya frontmatter padahal file
/// lama punya → prepend description lama (jangan sampah L0 karena rewrite).
pub fn rewrite_skill(dir: &Path, filename: &str, content: &str) -> Result<()> {
    if filename.contains('/') || filename.contains('\\') || !filename.ends_with(".md") {
        bail!("nama file skill tidak valid: {filename:?}");
    }
    let path = dir.join(filename);
    if !path.exists() {
        bail!("skill {filename} tidak ditemukan");
    }
    let old = fs::read_to_string(&path).unwrap_or_default();
    let had_fm = old.trim_start().starts_with("---");
    let content = if had_fm && !content.trim_start().starts_with("---") {
        let desc = parse_description(&old);
        format!("---\ndescription: {desc}\n---\n\n{}", content.trim_start())
    } else {
        content.to_string()
    };
    fs::write(&path, content).with_context(|| format!("gagal tulis {}", path.display()))?;
    Ok(())
}

/// Daftar semua skill (urut nama). Direktori belum ada → kosong. Description
/// dibaca dari isi file (L0) — jumlah skill single-user kecil, biaya read murah.
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
            let description =
                fs::read_to_string(e.path()).map(|c| parse_description(&c)).unwrap_or_default();
            Some(SkillMeta {
                name: filename.strip_suffix(".md")?.to_string(),
                filename,
                description,
                bytes,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Token keyword dari description untuk matching (lowercase, ≥MIN_TOKEN_LEN,
/// tanpa kata fungsi umum — description teks bebas, beda dengan nama skill).
fn desc_tokens(desc: &str) -> Vec<String> {
    desc.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= MIN_TOKEN_LEN)
        .filter(|t| !DESC_STOPWORDS.contains(t))
        .map(String::from)
        .collect()
}

/// Bagian system prompt untuk skills (Pilar 11): daftar nama + L0 description
/// selalu dimuat (LLM tahu topik skill yang belum dimuat — adopsi L0 OpenViking);
/// skill yang token-nya (nama ATAU description) cocok dengan pesan dimuat penuh
/// (max 3 file, cap 4K char).
pub fn section_for_prompt(dir: &Path, user_text: &str) -> (String, SkillsPromptStats) {
    let skills = list_skills(dir);
    if skills.is_empty() {
        return (
            "(belum ada skill tersimpan — gunakan save_skill setelah menyelesaikan \
             masalah non-trivial)"
                .into(),
            SkillsPromptStats::default(),
        );
    }

    let text_lower = user_text.to_lowercase();
    let mut matched: Vec<&SkillMeta> = skills
        .iter()
        .filter(|s| {
            let name_hit = s
                .name
                .split('-')
                .filter(|t| t.len() >= MIN_TOKEN_LEN)
                .any(|t| text_lower.contains(t));
            let desc_hit = desc_tokens(&s.description)
                .iter()
                .any(|t| text_lower.contains(t.as_str()));
            name_hit || desc_hit
        })
        .take(INJECT_MAX_FILES)
        .collect();

    let mut out = format!(
        "Skill tersimpan (format: nama — deskripsi L0). Muat penuh via \
         `read_file skills/<nama>.md` kalau relevan tapi belum dimuat di bawah:\n{}",
        skills
            .iter()
            .map(|s| format!("- {} — {}", s.name, s.description))
            .collect::<Vec<_>>()
            .join("\n")
    );

    if matched.is_empty() {
        return (
            out,
            SkillsPromptStats {
                listed: skills.len(),
                injected: 0,
            },
        );
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
    (
        out,
        SkillsPromptStats {
            listed: skills.len(),
            injected: matched.iter().take(INJECT_MAX_FILES).count(),
        },
    )
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
    fn parse_description_frontmatter() {
        let c = "---\ndescription: Renew sertifikat SSL via certbot\n---\n\n## Langkah";
        assert_eq!(parse_description(c), "Renew sertifikat SSL via certbot");
        // quoted value ikut di-strip
        let q = "---\ndescription: \"Deploy via CI\"\nother: x\n---\nbody";
        assert_eq!(parse_description(q), "Deploy via CI");
        // frontmatter tanpa description → fallback body
        let nod = "---\ntitle: x\n---\n\nBaris pertama body\nkedua";
        assert_eq!(parse_description(nod), "Baris pertama body");
    }

    #[test]
    fn parse_description_fallback() {
        // tanpa frontmatter: heading pertama minus '#'
        let h = "# Renew SSL nginx\n\nlangkah...";
        assert_eq!(parse_description(h), "Renew SSL nginx");
        // tanpa frontmatter: baris non-kosong pertama yang bukan heading
        let t = "\n\nCara deploy aplikasi ke VPS\n## Langkah";
        assert_eq!(parse_description(t), "Cara deploy aplikasi ke VPS");
        // sub-heading juga dibersihkan
        let sh = "## Sub judul\nbody";
        assert_eq!(parse_description(sh), "Sub judul");
    }

    #[test]
    fn save_writes_frontmatter() {
        let dir = std::env::temp_dir().join(format!("hermes-skills-fm-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        save_skill(
            &dir,
            "ssl",
            Some("Renew SSL certbot nginx"),
            "## Langkah\n1. certbot renew --dry-run dulu\n2. cek config nginx -t\ngotcha: port 80 dipakai apache",
        )
        .unwrap();
        let c = fs::read_to_string(dir.join("ssl.md")).unwrap();
        assert!(c.starts_with("---\ndescription: Renew SSL certbot nginx\n---\n"));
        assert_eq!(parse_description(&c), "Renew SSL certbot nginx");

        // description kosong → derive dari body (fallback), tetap frontmatter
        save_skill(&dir, "ssl", None, "Cara renew sertifikat SSL\n1. certbot renew --nginx\n2. cek expiry date\n3. reload nginx pelan-pelan").unwrap();
        let c = fs::read_to_string(dir.join("ssl.md")).unwrap();
        assert_eq!(parse_description(&c), "Cara renew sertifikat SSL");

        // description kepanjangan → ditolak
        assert!(
            save_skill(&dir, "ssl", Some(&"x".repeat(161)), "konten panjang ".repeat(20).as_str())
                .is_err()
        );

        // konten yang sudah bawa frontmatter → tidak dobel (di-strip, ditulis ulang)
        save_skill(
            &dir,
            "ssl",
            Some("baru"),
            "---\ndescription: lama\n---\n\n## Langkah\n1. langkah pertama yang panjang\n2. langkah kedua\n3. verifikasi hasil akhir selesai",
        )
        .unwrap();
        let c = fs::read_to_string(dir.join("ssl.md")).unwrap();
        assert_eq!(c.match_indices("---").count(), 2, "satu blok frontmatter saja");
        assert_eq!(parse_description(&c), "baru");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rewrite_preserves_frontmatter() {
        let dir = std::env::temp_dir().join(format!("hermes-skills-rw-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        save_skill(
            &dir,
            "ssl",
            Some("deskripsi penting"),
            "## Langkah\n1. certbot renew --dry-run\n2. stop apache dulu\ngotcha: port 80 dipakai apache",
        )
        .unwrap();
        // rewrite tanpa frontmatter → description lama di-prepend (safety net L0)
        rewrite_skill(&dir, "ssl.md", "## Langkah baru\n1. langkah\n2. langkah\n3. verifikasi").unwrap();
        let c = fs::read_to_string(dir.join("ssl.md")).unwrap();
        assert_eq!(parse_description(&c), "deskripsi penting");
        assert!(c.contains("## Langkah baru"));
        // rewrite yang sudah bawa frontmatter → tidak disentuh
        rewrite_skill(&dir, "ssl.md", "---\ndescription: versi baru\n---\n\n## V2\n1. x\n2. y\n3. z").unwrap();
        let c = fs::read_to_string(dir.join("ssl.md")).unwrap();
        assert_eq!(parse_description(&c), "versi baru");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_list_delete_roundtrip() {
        let dir = std::env::temp_dir().join(format!("hermes-skills-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        // anti-trivial
        assert!(save_skill(&dir, "x", None, "pendek").is_err());
        let ok = save_skill(
            &dir,
            "Renew SSL (nginx)",
            None,
            "## Langkah\n1. certbot renew\n2. test config\ngotcha: port 80 dipakai apache — stop dulu",
        )
        .unwrap();
        assert!(ok.contains("renew-ssl-nginx"));
        let listed = list_skills(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "renew-ssl-nginx");
        assert_eq!(listed[0].description, "Langkah"); // fallback: heading pertama

        // overwrite = update
        save_skill(&dir, "renew-ssl   nginx!!", None, "versi dua yang cukup panjang untuk lolos validasi minimal");
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
        save_skill(
            &dir,
            "renew-ssl-nginx",
            Some("Renew sertifikat SSL via certbot, termasuk urusan port 80"),
            body,
        )
        .unwrap();
        save_skill(
            &dir,
            "alat-tulis",
            Some("Deploy docker container ke VPS production"),
            body,
        )
        .unwrap();

        let (hit, stats) = section_for_prompt(&dir, "gimana cara renew sertifikat SSL di server ini?");
        assert!(hit.contains("Skill relevan"), "keyword match: {hit}");
        assert!(hit.contains("### skills/renew-ssl-nginx.md"));
        assert_eq!(stats.listed, 2);
        assert_eq!(stats.injected, 1);
        // daftar L0: nama — description
        assert!(hit.contains("renew-ssl-nginx — Renew sertifikat SSL via certbot"));
        // konten penuh yang tidak cocok tidak dimuat
        assert!(!hit.contains("### skills/alat-tulis"));

        // nama tidak menyebut topik, tapi description iya → match via token description
        let (via_desc, _) = section_for_prompt(&dir, "tolong deploy container production dong");
        assert!(via_desc.contains("### skills/alat-tulis.md"), "match via description: {via_desc}");

        let (miss, stats) = section_for_prompt(&dir, "halo, apa kabar?");
        assert!(miss.contains("renew-ssl-nginx —"), "daftar L0 selalu dimuat: {miss}");
        assert!(!miss.contains("###"));
        assert_eq!(stats.injected, 0);

        // token pendek (<3 char) tidak dipakai matching — "vps" cocok, "di" tidak
        // (kata fungsi "cara"/"dong" juga tidak match via description)
        assert!(section_for_prompt(&dir, "cek vps").0.contains("alat-tulis"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn desc_stopwords_do_not_match() {
        let dir = std::env::temp_dir().join(format!("hermes-skills-sw-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        save_skill(
            &dir,
            "abc-skill",
            Some("Dan untuk cara deploy dengan docker compose"),
            "## Langkah\n1. docker compose up\n2. verifikasi\ngotcha: volume harus ada dulu sebelum start",
        )
        .unwrap();
        // pesan hanya memuat kata fungsi → tidak ada injection palsu
        let (s, stats) = section_for_prompt(&dir, "halo dan apa kabar dengan kamu untuk hari ini");
        assert_eq!(stats.injected, 0, "stopword tidak boleh jadi match: {s}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_dir_section() {
        let (s, _) = section_for_prompt(Path::new("/tmp/definitely-no-skills-here"), "apa saja");
        assert!(s.contains("belum ada skill"));
    }
}
